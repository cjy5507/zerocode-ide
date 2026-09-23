/* The conversation view against the Claude Code extension's own webview
 * (2.1.280, the installed CLI's): what `docs/design/agent-conversation-
 * claude-code-grammar-20260915.md` §10 lists as the remaining differences,
 * each pinned where a person would see it (t-6323). The checks read the page
 * a browser actually laid out — a font is the face the engine drew with, a
 * fold is the height the row stood at — never the source's own spelling. */
import { openWindowTestPage } from "./window-boot.mjs";

/* A wire session's page opened on `history` — the page with the most roads
 * on it (turns, a live answer, the composer's wire) — painted once. */
export async function openConversation(page, history, { status = "idle", live = [] } = {}) {
  await page.evaluate(async ({ history, status, live }) => {
    window.__CONVERSATION__ = { status, live, turns: [] };
    window.__ANSWER__.wire_start = (args) => ({ id: 41, agent: args.agent, protocol: "claude-stream", version: "2.1.280", model: null, session: null });
    window.__ANSWER__.wire_log = (args) => {
      const held = window.__CONVERSATION__;
      return {
        found: true, skipped: false, next: held.turns.length, turns: held.turns.slice(args.after ?? 0),
        status: held.status, asks: [], live: held.live, agent: "claude", protocol: "claude-stream",
        models: [], modes: [], commands: [], version: "2.1.280",
      };
    };
    window.__ANSWER__.wire_stop = () => null;
    await openWirePage("claude", "/tmp/zerocode-window-test", { history });
    await pollHelperPages();
    await window.__PAINTED__();
  }, { history, status, live });
}

/* The faces the engine drew a node's text with (CDP, the renderer's own
 * answer) — not the declared list, which names faces that may not exist. */
async function drawnFaces(cdp, selector) {
  const doc = await cdp.send("DOM.getDocument", { depth: 0 });
  const found = await cdp.send("DOM.querySelector", { nodeId: doc.root.nodeId, selector });
  if (!found.nodeId) return null;
  const { fonts } = await cdp.send("CSS.getPlatformFontsForNode", { nodeId: found.nodeId });
  return fonts.map((one) => one.familyName);
}

/* A0 — the face. The extension's panel wears VS Code's UI face
 * (`--vscode-font-family`, the platform's sans), and so must this page,
 * whatever the person chose: a chosen face that is not installed — the
 * default `Geist` is a name, never a shipped file — must fall through to the
 * platform's sans, not to the engine's default face (WebKit's Times, and for
 * Hangul AppleMyungjo: the serif the person saw, 2026-09-23). Which face is
 * the engine's default depends on the engine and the page's language, so the
 * page carries two controls beside the words: the same Latin word in a face
 * that does not exist (what the engine falls to) and in the platform's sans
 * (what the extension wears). Five ways a choice can arrive: the default,
 * none, a face this machine lacks, the same typed with quotes, and a list. */
export async function testConversationFont(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("DOM.enable");
    await cdp.send("CSS.enable");
    await openConversation(page, [
      { role: "user", text: "대화 뷰가 느려요 — why is it slow?" },
      { role: "assistant", text: "원인은 paint가 폴마다 목록을 다시 세운 것입니다 — the list was rebuilt on every poll." },
      { role: "tool", text: "Read · ui/shell.js", tool: { call_id: "r1", name: "Read", input: "ui/shell.js", is_error: false } },
    ]);
    await page.evaluate(async () => {
      const list = document.querySelector("#worker-view .helper-turns");
      const controls = [
        ["conversation-font-engine", "\"No Such Face 6323 Control\""],
        ["conversation-font-system", "-apple-system, BlinkMacSystemFont, sans-serif"],
      ];
      for (const [id, face] of controls) {
        const control = document.createElement("span");
        control.id = id;
        control.style.fontFamily = face;
        control.textContent = "Read";
        list.prepend(control);
      }
      // The engine answers for text it has laid out: a frame first.
      await window.__PAINTED__();
    });
    const engine = await drawnFaces(cdp, "#conversation-font-engine");
    const system = await drawnFaces(cdp, "#conversation-font-system");
    const cases = {};
    const choices = [
      ["default", "Geist"],
      ["unset", ""],
      ["missing", "No Such Face 6323"],
      ["quoted", "\"No Such Face 6323\""],
      ["listed", "No Such Face 6323, Also Missing 6323"],
    ];
    for (const [name, choice] of choices) {
      const declared = await page.evaluate(async (choice) => {
        appFontFamily = choice;
        applyAppFontFamily();
        await window.__PAINTED__();
        const prose = document.querySelector("#worker-view .helper-turn.is-assistant .helper-said");
        return {
          choice: document.documentElement.style.getPropertyValue("--font-ui-choice"),
          prose: getComputedStyle(prose).fontFamily,
          tool: getComputedStyle(document.querySelector("#worker-view .helper-tool-name")).fontFamily,
        };
      }, choice);
      const drawn = {
        prose: await drawnFaces(cdp, "#worker-view .helper-turn.is-assistant .helper-said p"),
        tool: await drawnFaces(cdp, "#worker-view .helper-tool-name"),
        user: await drawnFaces(cdp, "#worker-view .helper-turn.is-user .helper-said"),
      };
      // The Latin words stand in the platform's sans: the tool's name is all
      // Latin, and the prose and the person's words carry Latin beside Hangul.
      const latinInSans = drawn.tool?.join() === system?.join() &&
        [drawn.prose, drawn.user].every((faces) => faces?.includes(system?.[0]) === true);
      const fellToEngine = engine?.[0] !== system?.[0] &&
        Object.values(drawn).some((faces) => faces?.[0] === engine?.[0]);
      // The declared list ends in the platform's sans whatever came first.
      const chained = [declared.prose, declared.tool].every((list) =>
        list.includes("-apple-system") && /sans-serif\s*$/.test(list));
      cases[name] = { declared, drawn, latinInSans, fellToEngine, chained };
    }
    ok(
      "A0: the conversation draws its Latin words in the platform's sans whatever face the person chose — the default Geist (a name, not a shipped file), none, a face this machine lacks, the same typed in quotes, and a typed list all fall through to the system sans, never to the engine's default face (WebKit: Times / AppleMyungjo); the declared list keeps the system chain after the choice, a quoted name is that name, and a typed list stays a list",
      engine?.length > 0 && system?.length > 0 &&
        Object.values(cases).every((one) => one.latinInSans && !one.fellToEngine && one.chained) &&
        cases.quoted.declared.choice === cases.missing.declared.choice &&
        cases.listed.declared.choice.split(",").length === 2,
      JSON.stringify({ engine, system, cases }),
    );
    ok("A0: the font cases raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A1 — the folds. The extension keeps every long thing to a few lines and
 * lets a press unfold it (2.1.280 webview): a tool body's row stops at 60px
 * behind a 10px fade (`toolBodyRowContent`, and a row is "long" past 3 lines
 * or 250 characters — `cN`), a diff stops at 200px behind a 30px fade
 * (`Math.min(200, contentHeight + 20)`, "Click to expand"), and what the
 * person said stops at 60px behind a 50px fade with "Show more" / "Show
 * less" (`EV0`, `maxHeight: 60`). Short things stand whole. Here each fold is
 * a row's own (Focus view folds a turn; these fold a row), the heights come
 * from tokens, a diff builds only the rows its clip shows until it is
 * opened, and a quiet poll touches nothing. */
export async function testConversationFolds(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const lines = (count, word) => Array.from({ length: count }, (_, at) => `${word} ${at}: ${"결과 줄 ".repeat(3)}`).join("\n");
    const diff = (count) => Array.from({ length: count }, (_, at) => ({
      kind: at % 3 === 0 ? "del" : "add", text: `const value_${at} = compute(${at});`, old: null, new: null,
    }));
    await openConversation(page, [
      { role: "user", text: lines(40, "브리핑") },
      { role: "user", text: "짧은 부탁" },
      { role: "tool", text: "Bash · cargo test", tool: { call_id: "b1", name: "Bash", input: "cargo test", is_error: false } },
      { role: "tool_result", text: lines(200, "test"), tool: { call_id: "b1", is_error: false } },
      { role: "tool", text: "Bash · ls", tool: { call_id: "b2", name: "Bash", input: "ls", is_error: false } },
      { role: "tool_result", text: "a.rs\nb.rs", tool: { call_id: "b2", is_error: false } },
      { role: "tool", text: "Edit · src/big.rs", tool: { call_id: "e1", name: "Edit", input: "{}", is_error: false,
        edits: [{ path: "src/big.rs", lines: diff(120) }] } },
      { role: "tool_result", text: "updated", tool: { call_id: "e1", is_error: false } },
      { role: "tool", text: "Edit · src/small.rs", tool: { call_id: "e2", name: "Edit", input: "{}", is_error: false,
        edits: [{ path: "src/small.rs", lines: diff(5) }] } },
      { role: "tool_result", text: "updated", tool: { call_id: "e2", is_error: false } },
      { role: "assistant", text: "끝났습니다." },
    ]);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const face = document.querySelector("#worker-view");
      const list = face.querySelector(".helper-turns");
      const token = (name) => parseFloat(getComputedStyle(face).getPropertyValue(name));
      const more = t("worker.showMore", "더 보기");
      const less = t("worker.showLess", "접기");
      seen.tokens = { tool: token("--chat-tool-clip-h"), diff: token("--chat-diff-clip-h"), user: token("--chat-user-clip-h") };
      const users = [...list.querySelectorAll(":scope > .helper-turn.is-user")];
      const tools = [...list.querySelectorAll(":scope > .helper-turn.is-tool")];
      const expandOf = (row) => row.querySelector(".helper-expand");
      const height = (node) => node?.getBoundingClientRect().height ?? -1;
      const content = (node) => {
        const style = getComputedStyle(node);
        return height(node) - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom) -
          parseFloat(style.borderTopWidth) - parseFloat(style.borderBottomWidth);
      };
      await window.__PAINTED__();
      // The person's long words: 60px of them, the fade, the door.
      const long = users[0].querySelector(".helper-said");
      seen.userClipped = long.classList.contains("is-clipped");
      seen.userHeight = Math.round(content(long));
      seen.userDoor = expandOf(users[0])?.textContent ?? "";
      // The door is the box's neighbour, never its words: what is copied or
      // read out of the box is what the person said.
      seen.userText = long.textContent.startsWith("브리핑 0:") && !long.textContent.includes(more);
      expandOf(users[0])?.click();
      await window.__PAINTED__();
      seen.userOpen = long.classList.contains("is-open") && Math.round(content(long)) > seen.userHeight * 5;
      seen.userDoorOpen = expandOf(users[0])?.textContent ?? "";
      expandOf(users[0])?.click();
      await window.__PAINTED__();
      seen.userClosedAgain = !long.classList.contains("is-open") && Math.round(content(long)) === seen.userHeight;
      seen.shortUser = !users[1].querySelector(".helper-said").classList.contains("is-clipped") && expandOf(users[1]) === null;
      // A long output: the body stands (no closed fold to press first), its
      // OUT row stops at the clip and says it has more.
      const [bash, ls, big, small] = tools;
      seen.oldFold = list.querySelectorAll("details.helper-tool-more").length;
      const out = bash.querySelector(".helper-tool-output");
      seen.bodyShown = out !== null && out.checkVisibility();
      seen.outClipped = out?.classList.contains("is-clipped") === true;
      seen.outHeight = out ? Math.round(content(out)) : -1;
      seen.outDoor = out?.parentElement?.querySelector(".helper-expand")?.textContent ?? bash.querySelector(".helper-expand")?.textContent ?? "";
      bash.querySelector(".helper-expand")?.click();
      await window.__PAINTED__();
      // Opened, it is the well it always was: taller than the clip, and it
      // scrolls on its own rather than lengthening the page by 200 lines.
      seen.outOpen = out ? Math.round(content(out)) > seen.tokens.tool * 3 && getComputedStyle(out).overflowY === "auto" : false;
      bash.querySelector(".helper-expand")?.click();
      await window.__PAINTED__();
      // A short output stands whole, with no door.
      const lsOut = ls.querySelector(".helper-tool-output");
      seen.shortOut = (lsOut === null || !lsOut.classList.contains("is-clipped")) && ls.querySelector(".helper-expand") === null;
      // A long diff: 200px, only the rows the clip shows are built, the door.
      const bigRows = big.querySelector(".helper-tool-diff-rows");
      seen.diffClipped = bigRows?.classList.contains("is-clipped") === true;
      // The extension's diff BOX is 200px tall — edge and padding included.
      seen.diffHeight = bigRows ? Math.round(height(bigRows)) : -1;
      seen.diffBuilt = bigRows?.querySelectorAll(".diff-line").length ?? -1;
      seen.diffDoor = big.querySelector(".helper-expand")?.textContent ?? "";
      big.querySelector(".helper-expand")?.click();
      await window.__PAINTED__();
      seen.diffOpenBuilt = bigRows?.querySelectorAll(".diff-line").length ?? -1;
      seen.diffOpenHeight = bigRows ? Math.round(height(bigRows)) : -1;
      seen.diffOpenScrolls = bigRows ? getComputedStyle(bigRows).overflowY === "auto" : false;
      seen.diffDoorOpen = big.querySelector(".helper-expand")?.textContent ?? "";
      big.querySelector(".helper-expand")?.click();
      await window.__PAINTED__();
      seen.diffClosedAgain = bigRows ? Math.round(height(bigRows)) === seen.diffHeight : false;
      const smallRows = small.querySelector(".helper-tool-diff-rows");
      seen.smallDiff = !smallRows?.classList.contains("is-clipped") && smallRows?.querySelectorAll(".diff-line").length === 5 &&
        small.querySelector(".helper-expand") === null;
      // A quiet poll writes nothing.
      const watch = new MutationObserver(() => {});
      watch.observe(list, { childList: true, subtree: true, characterData: true, attributes: true });
      await pollHelperPages();
      await window.__PAINTED__();
      seen.quiet = watch.takeRecords().length;
      watch.disconnect();
      seen.more = more;
      seen.less = less;
      return seen;
    });
    const near = (value, want) => Math.abs(value - want) <= 1;
    ok(
      "A1: what the person said stops at the extension's 60px (a token) with 「더 보기」, opens whole and closes back with 「접기」, and a short message stands whole with no door",
      seen.tokens.user === 60 && seen.userClipped && near(seen.userHeight, seen.tokens.user) &&
        seen.userDoor === seen.more && seen.userText && seen.userOpen && seen.userDoorOpen === seen.less &&
        seen.userClosedAgain && seen.shortUser,
      JSON.stringify(seen),
    );
    ok(
      "A1: a long tool output stands in its body without a fold to press first — the OUT row stops at the extension's 60px (a token) with 「더 보기」 and opens into its own scrolling well — while a short output stands whole with no door",
      seen.tokens.tool === 60 && seen.oldFold === 0 && seen.bodyShown && seen.outClipped &&
        near(seen.outHeight, seen.tokens.tool) && seen.outDoor === seen.more && seen.outOpen && seen.shortOut,
      JSON.stringify(seen),
    );
    ok(
      "A1: a long diff's box stops at the extension's 200px (a token) with only the rows that clip shows built, 「더 보기」 builds the rest and opens it into its own scrolling well, 「접기」 closes it back — a short diff stands whole — and a quiet poll writes nothing",
      seen.tokens.diff === 200 && seen.diffClipped && near(seen.diffHeight, seen.tokens.diff) &&
        seen.diffBuilt > 0 && seen.diffBuilt < 20 && seen.diffDoor === seen.more &&
        seen.diffOpenBuilt === 120 && seen.diffOpenHeight > seen.tokens.diff && seen.diffOpenScrolls && seen.diffDoorOpen === seen.less &&
        seen.diffClosedAgain && seen.smallDiff && seen.quiet === 0,
      JSON.stringify(seen),
    );
    ok("A1: the folds raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

