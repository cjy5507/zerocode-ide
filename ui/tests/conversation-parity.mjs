/* The conversation view against the Claude Code extension's own webview
 * (2.1.280, the installed CLI's): what `docs/design/agent-conversation-
 * claude-code-grammar-20260915.md` §10 lists as the remaining differences,
 * each pinned where a person would see it (t-6323). The checks read the page
 * a browser actually laid out — a font is the face the engine drew with, a
 * fold is the height the row stood at — never the source's own spelling. */
import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { chromium, createWindowServer, openWindowTestPage } from "./window-boot.mjs";
import { FIXTURE_PNG, conversationFixture, openFixtureConversation, webkitType } from "./conversation-perf.mjs";

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
        status: held.status, asks: held.asks ?? [], live: held.live, agent: "claude", protocol: "claude-stream",
        models: [], mode: held.mode ?? null, modes: held.modes ?? [], commands: [], version: "2.1.280",
        tasks: held.tasks ?? [],
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

/* A2 — a path is a door to the file tab. The extension's tool header links
 * the file a call read or wrote (`fileToolHeader`, 2.1.280): a Read opens at
 * the lines it read and says them — "(lines a-b)" / "(from line a)" — an
 * Edit at the text it wrote (`searchText`), a Write at the top; Grep and
 * Bash name no file and link nothing. Inside an answer, a markdown link
 * `path:12` / `path#L20-L30` opens at its line (`wg`). Here the door is the
 * document viewer's (`openPath`), measured from the session's checkout, and
 * a bare path the answer names keeps its `:line` too. Where a read began is
 * the CLI's own count: Claude Code's `offset` IS the first line it returned
 * (measured 2026-09-23: offset 10930 → the result opens `10930→`), zo's is
 * 0-based (`read_file`'s schema) — a catalog fact, never guessed here. */
export async function testConversationPaths(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const root = "/tmp/zerocode-window-test";
    await page.evaluate(() => {
      const lines = Array.from({ length: 120 }, (_, at) => `line ${at + 1}`);
      window.__READS__ = [];
      window.__ANSWER__.read_text_file = (args) => {
        window.__READS__.push(args.path);
        const text = args.path.endsWith("edit.rs")
          ? [...lines.slice(0, 40), "let fixed = true;", ...lines.slice(41)].join("\n")
          : lines.join("\n");
        return { text, version: 1, missing: false };
      };
    });
    await openConversation(page, [
      { role: "tool", text: `Read · ${root}/src/app.rs`, tool: { call_id: "r1", name: "Read", is_error: false,
        input: JSON.stringify({ file_path: `${root}/src/app.rs`, offset: 11, limit: 50 }, null, 2),
        file: { path: `${root}/src/app.rs`, offset: 11, limit: 50 } } },
      { role: "tool_result", text: "    11→line 11", tool: { call_id: "r1", is_error: false } },
      { role: "tool", text: `Edit · ${root}/src/edit.rs`, tool: { call_id: "e1", name: "Edit", is_error: false,
        input: "{}", file: { path: `${root}/src/edit.rs`, search: "let fixed = true;" },
        edits: [{ path: `${root}/src/edit.rs`, lines: [{ kind: "add", text: "let fixed = true;", old: null, new: null }] }] } },
      { role: "tool_result", text: "updated", tool: { call_id: "e1", is_error: false } },
      { role: "tool", text: "Grep · needle", tool: { call_id: "g1", name: "Grep", is_error: false, input: "{}" } },
      { role: "tool_result", text: "3 matches", tool: { call_id: "g1", is_error: false } },
      { role: "assistant", text: "보세요: [app](src/app.rs:12), [범위](src/lib.rs#L20-L30), 그리고 ui/shell.js:42 에 있습니다." },
    ]);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const face = document.querySelector("#worker-view");
      const tools = [...face.querySelectorAll(".helper-turn.is-tool")];
      const doorOf = (row) => row.querySelector(".helper-tool-arg[role=button]");
      const openedAt = async (press) => {
        press();
        for (let beat = 0; beat < 6; beat += 1) await window.__PAINTED__();
        const editor = window.__EDITOR__();
        const tab = tabs.find((one) => one.id === activeTabId);
        return {
          tab: tab?.id ?? null,
          line: editor ? editor.state.doc.lineAt(editor.state.selection.main.head).number : null,
        };
      };
      const [read, edit, grep] = tools;
      seen.readDoor = doorOf(read) !== null;
      seen.readWhere = read.querySelector(".helper-tool-where")?.textContent ?? "";
      seen.wantReadWhere = t("worker.readLines", "({{from}}–{{to}}행)", { from: 11, to: 60 });
      seen.grepDoor = doorOf(grep) === null && grep.querySelector(".helper-tool-where") === null;
      const pageTab = activeTabId;
      seen.read = await openedAt(() => doorOf(read)?.click());
      setActiveTab(pageTab);
      await window.__PAINTED__();
      seen.edit = await openedAt(() => doorOf(edit)?.click());
      setActiveTab(pageTab);
      await window.__PAINTED__();
      const links = [...face.querySelectorAll(".helper-turn.is-assistant .md-link")];
      const byLabel = (label) => links.find((one) => one.textContent.includes(label));
      seen.mdLine = await openedAt(() => byLabel("app")?.click());
      setActiveTab(pageTab);
      await window.__PAINTED__();
      seen.mdRange = await openedAt(() => byLabel("범위")?.click());
      setActiveTab(pageTab);
      await window.__PAINTED__();
      seen.bare = await openedAt(() => byLabel("ui/shell.js:42")?.click());
      seen.reads = window.__READS__;
      return seen;
    });
    ok(
      "A2: a Read row's file is a door that says the lines it read and opens the file tab at the first of them — the CLI's own count — an Edit row's opens where its new text stands, and a Grep row names no file and links nothing",
      seen.readDoor && seen.readWhere === seen.wantReadWhere && seen.grepDoor &&
        seen.read.tab === `file:${root}/src/app.rs` && seen.read.line === 11 &&
        seen.edit.tab === `file:${root}/src/edit.rs` && seen.edit.line === 41,
      JSON.stringify(seen),
    );
    ok(
      "A2: inside an answer a markdown link opens its file at its line (`path:12`, `path#L20-L30`), and a bare path the answer names keeps its `:line`",
      seen.mdLine.tab === `file:${root}/src/app.rs` && seen.mdLine.line === 12 &&
        seen.mdRange.tab === `file:${root}/src/lib.rs` && seen.mdRange.line === 20 &&
        seen.bare.tab === `file:${root}/ui/shell.js` && seen.bare.line === 42,
      JSON.stringify(seen),
    );
    ok("A2: the doors raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A3 — the keyboard. The extension's prompt keys (2.1.280 webview): a plain
 * Esc interrupts the turn from anywhere on the panel (`zU0` on body; a popup
 * takes it first, a permission card too); Shift+Tab steps the permission
 * mode through `d6` — [Don't ask while in it] Manual, Edit automatically,
 * Plan, Auto, Bypass permissions — in the CLI's own words; ArrowUp at the
 * very start of the box recalls the previous prompt (the session's, newest
 * first, queued ones included) and ArrowDown at the very end walks back to
 * the draft; Enter sends, Shift+Enter or Ctrl+J breaks the line. The wire's
 * interrupt is its own request; a pane's is the key its CLI names
 * (`interrupt_key`: Claude Code and zo say "esc to interrupt" on their own
 * screens). The stop button is the same interrupt. */
export async function testConversationKeys(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(() => {
      window.__CALLS__ = [];
      for (const name of ["wire_interrupt", "wire_set_mode", "wire_send", "term_key"]) {
        const before = window.__ANSWER__[name];
        window.__ANSWER__[name] = (args) => {
          window.__CALLS__.push([name, args]);
          return typeof before === "function" ? before(args) : null;
        };
      }
    });
    await openConversation(page, [
      { role: "user", text: "첫째 부탁" },
      { role: "assistant", text: "네." },
      { role: "user", text: "둘째 부탁" },
      { role: "assistant", text: "알겠습니다." },
    ], { status: "working" });
    const seen = await page.evaluate(async () => {
      const seen = {};
      const settle = async () => {
        await pollHelperPages();
        for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      };
      const face = document.querySelector("#worker-view");
      const box = face.querySelector(".worker-composer-box");
      const press = (target, key, extra = {}) => target.dispatchEvent(new KeyboardEvent("keydown", {
        key, bubbles: true, cancelable: true, ...extra,
      }));
      const calls = (name) => window.__CALLS__.filter(([called]) => called === name).map(([, args]) => args);
      const tab = tabs.find((one) => one.id === activeTabId);
      const log = window.__CONVERSATION__;
      log.status = "working";
      log.modes = ["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"].map((id) => ({ id, name: id }));
      await settle();

      // Esc from the list (not only the box) interrupts a working turn.
      box.blur();
      press(face.querySelector(".helper-turns"), "Escape");
      await settle();
      seen.escWorking = JSON.stringify(calls("wire_interrupt"));
      // A modifier, a repeat or a composition is not the interrupt.
      press(box, "Escape", { shiftKey: true });
      press(box, "Escape", { repeat: true });
      press(box, "Escape", { isComposing: true });
      await settle();
      seen.escIgnored = calls("wire_interrupt").length;
      // The stop button is the same interrupt (it threw before: the road had none).
      face.querySelector(".worker-composer-send.is-stop")?.click();
      await settle();
      seen.stopInterrupts = calls("wire_interrupt").length;
      // Idle: nothing to interrupt.
      log.status = "idle";
      await settle();
      press(box, "Escape");
      await settle();
      seen.escIdle = calls("wire_interrupt").length;

      // Shift+Tab: the extension's order, the CLI's words, from the box.
      const chipWords = () => face.querySelector(".worker-composer-mode .worker-composer-pill-words")?.textContent ?? "";
      const steps = [];
      const walk = ["default", "acceptEdits", "plan", "auto", "bypassPermissions"];
      for (const mode of walk) {
        log.mode = mode;
        await settle();
        const words = chipWords();
        const before = calls("wire_set_mode").length;
        press(box, "Tab", { shiftKey: true });
        await settle();
        steps.push([mode, words, calls("wire_set_mode")[before]?.mode ?? null]);
      }
      seen.steps = steps;
      log.mode = "dontAsk";
      await settle();
      seen.dontAskWords = chipWords();
      const beforeDontAsk = calls("wire_set_mode").length;
      press(box, "Tab", { shiftKey: true });
      await settle();
      seen.dontAskNext = calls("wire_set_mode")[beforeDontAsk]?.mode ?? null;

      // ArrowUp at the start recalls, newest first; ArrowDown at the end walks back.
      box.focus();
      box.value = "쓰던 글";
      box.dispatchEvent(new Event("input", { bubbles: true }));
      box.setSelectionRange(2, 2);
      press(box, "ArrowUp");
      seen.midCaret = box.value;
      box.setSelectionRange(0, 0);
      press(box, "ArrowUp");
      seen.up1 = box.value;
      box.setSelectionRange(0, 0);
      press(box, "ArrowUp");
      seen.up2 = box.value;
      box.setSelectionRange(0, 0);
      press(box, "ArrowUp");
      seen.upPastOldest = box.value;
      box.setSelectionRange(box.value.length, box.value.length);
      press(box, "ArrowDown");
      seen.down1 = box.value;
      box.setSelectionRange(box.value.length, box.value.length);
      press(box, "ArrowDown");
      seen.downToDraft = box.value;

      // Ctrl+J breaks the line where the caret stands; Enter sends.
      box.value = "한 줄";
      box.dispatchEvent(new Event("input", { bubbles: true }));
      box.setSelectionRange(1, 1);
      press(box, "j", { ctrlKey: true });
      seen.ctrlJ = JSON.stringify(box.value);
      press(box, "Enter");
      await settle();
      seen.sent = JSON.stringify(calls("wire_send").map((args) => args.text));

      // A pane's conversation: Esc and the stop button press the key its CLI
      // names (Claude Code: "esc to interrupt"), not the terminal's ^C.
      const term = await openTermTab({ placement: "tab" });
      paneAgents.set(term, "claude");
      hookStates.set(term, "working");
      window.__ANSWER__.pane_log = () => ({ found: true, next: 1, skipped: false, more: false, folded: false,
        turns: [{ role: "user", text: "판의 부탁" }] });
      await setPaneChat(term, true);
      for (let beat = 0; beat < 4; beat += 1) await window.__PAINTED__();
      const held = paneChats.get(term);
      const keysFor = () => calls("term_key").filter((args) => args.term === term).map((args) => args.press);
      press(held.host.querySelector(".helper-turns"), "Escape");
      await settle();
      seen.paneEsc = JSON.stringify(keysFor());
      held.host.querySelector(".worker-composer-send.is-stop")?.click();
      await settle();
      seen.paneStop = JSON.stringify(keysFor());
      return seen;
    });
    ok(
      "A3: a plain Esc anywhere on the conversation interrupts the turn — down the wire, or on a pane by the key its CLI names (Esc, not ^C) — while a modifier, a repeat, a composition or an idle turn does not, and the stop button is that same interrupt",
      seen.escWorking === JSON.stringify([{ id: 41 }]) && seen.escIgnored === 1 && seen.stopInterrupts === 2 &&
        seen.escIdle === 2 &&
        seen.paneEsc === JSON.stringify([{ key: "Escape", ctrl: false, alt: false }]) &&
        seen.paneStop === JSON.stringify([{ key: "Escape", ctrl: false, alt: false }, { key: "Escape", ctrl: false, alt: false }]),
      JSON.stringify(seen),
    );
    ok(
      "A3: Shift+Tab steps the permission mode in the extension's order and the chip says the CLI's words — Manual → Edit automatically → Plan → Auto → Bypass permissions → Manual — and Don't ask steps out to Manual",
      JSON.stringify(seen.steps) === JSON.stringify([
        ["default", "Manual", "acceptEdits"],
        ["acceptEdits", "Edit automatically", "plan"],
        ["plan", "Plan", "auto"],
        ["auto", "Auto", "bypassPermissions"],
        ["bypassPermissions", "Bypass permissions", "default"],
      ]) && seen.dontAskWords === "Don't ask" && seen.dontAskNext === "default",
      JSON.stringify(seen),
    );
    ok(
      "A3: ArrowUp at the very start of the box recalls the session's prompts newest first and stops at the oldest, ArrowDown at the very end walks back to the draft, a caret mid-text moves as text does, Ctrl+J breaks the line and Enter sends",
      seen.midCaret === "쓰던 글" && seen.up1 === "둘째 부탁" && seen.up2 === "첫째 부탁" &&
        seen.upPastOldest === "첫째 부탁" && seen.down1 === "둘째 부탁" && seen.downToDraft === "쓰던 글" &&
        seen.ctrlJ === JSON.stringify("한\n 줄") && seen.sent === JSON.stringify(["한\n 줄"]),
      JSON.stringify(seen),
    );
    ok("A3: the keys raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A4 — while the turn is out. The extension's spinner row (`Ke`, 2.1.280):
 * the glyph turns every 120 ms, and the word is one of the CLI's own verbs
 * (84 of them, `tD1` — the CLI's screen carries the same list) picked at
 * random and picked again at 2 s, 5 s, 10 s and every 5 s after, written
 * `Verb...`; each new verb is revealed by a sweep (`j75`: every 40 ms a
 * four-character window moves right — `▌`, two of `.`/`_`/the letter, then
 * the letter). No elapsed time and no token count stand beside it — the head
 * carries the time, the meter the context — and no caret trails the answer:
 * the extension's only `▌` is this sweep. A CLI the catalog gives no verbs
 * keeps its one word. */
export async function testConversationStatus(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, [{ role: "user", text: "시작" }], { status: "working" });
    const seen = await page.evaluate(async () => {
      const seen = {};
      const face = document.querySelector("#worker-view");
      const word = () => face.querySelector(".helper-status-word")?.textContent ?? "";
      const verbs = installedAgents().find((row) => row.id === "claude")?.spinner_verbs ?? [];
      seen.verbCount = verbs.length;
      // The first verb sweeps in over a blank field; wait for it to settle.
      const isVerb = (text) => verbs.some((verb) => text === `${verb}...`);
      for (let beat = 0; beat < 240 && !isVerb(word()); beat += 1) await window.__PAINTED__();
      const first = word();
      seen.firstIsVerb = isVerb(first);
      // The sweep, as a pure step: the window's lead is the bar, its tail
      // the target, and what the window has passed is the new word.
      const step = (from, to, at) => (typeof statusRevealStep === "function"
        ? statusRevealStep(from, to, at, (choices) => choices[0])
        : null);
      seen.sweep = [0, 1, 2, 3, 4, 7].map((at) => step("Old...", "New...", at));
      // A new pick within the extension's first beat (2 s), revealed by the
      // sweep — a pick may land on the same verb, as the extension's may.
      const bars = [];
      const started = performance.now();
      while (performance.now() - started < 2600 && bars.length === 0) {
        await new Promise((done) => requestAnimationFrame(done));
        if (word().includes("▌")) bars.push(word());
      }
      for (let beat = 0; beat < 240 && !isVerb(word()); beat += 1) await window.__PAINTED__();
      seen.swept = bars.length > 0;
      seen.secondIsVerb = isVerb(word());
      seen.delays = [0, 1, 2, 3, 9].map((picks) => (typeof statusVerbDelay === "function" ? statusVerbDelay(picks) : null));
      // No caret trails a streaming answer.
      window.__CONVERSATION__.live = [{ role: "assistant", text: "흐르는 답" }];
      await pollHelperPages();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      seen.noCaret = !face.querySelector(".is-streaming")?.textContent.includes("▌");
      return seen;
    });
    ok(
      "A4: while the turn is out the status says one of Claude Code's own 84 verbs as `Verb...`, picks again at the extension's beats (2 s, 5 s, 10 s, then every 5 s) and reveals each new verb with the sweep — `▌`, two of `.`/`_`/the letter, then the letter — and no caret trails the answer",
      seen.verbCount === 84 && seen.firstIsVerb && seen.swept && seen.secondIsVerb &&
        JSON.stringify(seen.delays) === JSON.stringify([2000, 3000, 5000, 5000, 5000]) &&
        JSON.stringify(seen.sweep) === JSON.stringify(["▌ld...", ".▌d...", "..▌...", "N..▌..", "Ne..▌.", "New..."]) &&
        seen.noCaret,
      JSON.stringify(seen),
    );
    ok("A4: the status raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A5 — the scroll. The extension's list (2.1.280 `VG0`, `lF1`): within 50px
 * of its foot (`TF`) and not scrolled away, new words keep it at its foot.
 * Leaving is an intent, not a distance: an upward wheel or key (ArrowUp,
 * PageUp, Home, Shift+Space) leaves at once however near the foot the list
 * stood, and a move up the page did not cause (the thumb) leaves too; a move
 * back into the last 50px comes back. A wheel a box inside the list can still
 * scroll is that box's. Sending comes back and takes the list to its foot
 * (`scrollToBottomOnSend`, on by default). The extension grows no "jump to
 * latest" button; this page grows one because the person asked for it
 * (사용자 요청 09-24, t-6824 — `testConversationFoot`), hidden at the foot. */
export async function testConversationScroll(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const history = [];
    for (let at = 0; at < 40; at += 1) {
      history.push({ role: "user", text: `부탁 ${at}` });
      history.push({ role: "assistant", text: `답 ${at}: ${"긴 문장이 여러 줄로 이어진다. ".repeat(6)}` });
    }
    await openConversation(page, history, { status: "working", live: [{ role: "assistant", text: "흐르는" }] });
    // The list's place once it has stopped moving (a wheel may glide).
    const still = () => page.evaluate(async () => {
      const list = document.querySelector("#worker-view .helper-turns");
      let last = -1;
      let same = 0;
      for (let beat = 0; beat < 180 && same < 5; beat += 1) {
        await new Promise((done) => requestAnimationFrame(done));
        same = list.scrollTop === last ? same + 1 : 0;
        last = list.scrollTop;
      }
      return { top: Math.round(list.scrollTop), gap: Math.round(list.scrollHeight - list.scrollTop - list.clientHeight) };
    });
    let grown = 0;
    const grow = async () => {
      grown += 1;
      await page.evaluate(async (grown) => {
        window.__CONVERSATION__.live = [{ role: "assistant", text: `흐르는 ${"새로 온 말이 한 줄 더 붙는다. ".repeat(30 * grown)}` }];
        await pollHelperPages();
        for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      }, grown);
      return still();
    };
    const box = await page.evaluate(() => {
      const rect = document.querySelector("#worker-view .helper-turns").getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    });
    await page.mouse.move(box.x, box.y);
    const seen = {};
    seen.start = await grow();
    // A small wheel upward, still inside the last 50px, leaves at once.
    await page.mouse.wheel(0, -30);
    seen.wheeledUp = await still();
    seen.afterWheelUp = await grow();
    // Back down into the last 50px: the words are followed again.
    await page.mouse.wheel(0, 100000);
    await still();
    seen.afterWheelDown = await grow();
    // A key upward leaves by itself, before anything moved.
    await page.evaluate(() => {
      const list = document.querySelector("#worker-view .helper-turns");
      list.focus();
      list.dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp", bubbles: true, cancelable: true }));
    });
    seen.keyedUp = await still();
    seen.afterKeyUp = await grow();
    // The End key and a move to the foot come back.
    await page.evaluate(() => {
      const list = document.querySelector("#worker-view .helper-turns");
      list.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true }));
      list.scrollTop = list.scrollHeight;
    });
    await still();
    seen.afterEnd = await grow();
    // The thumb dragged up — no key, no wheel — leaves too.
    await page.evaluate(() => {
      document.querySelector("#worker-view .helper-turns").scrollTop -= 300;
    });
    seen.dragged = await still();
    seen.afterDrag = await grow();
    // A box inside the list that can still scroll takes the wheel for itself.
    seen.inner = await page.evaluate(() => {
      const list = document.querySelector("#worker-view .helper-turns");
      const well = document.createElement("div");
      well.style.cssText = "overflow-y: auto; height: 40px;";
      well.innerHTML = "<div style='height: 400px'>안</div>";
      list.appendChild(well);
      well.scrollTop = 100;
      const inside = well.firstElementChild;
      const answer = typeof innerScrolls === "function"
        ? { up: innerScrolls(list, inside, true), down: innerScrolls(list, inside, false), outside: innerScrolls(list, list, true) }
        : null;
      well.remove();
      return answer;
    });
    // Sending comes back and takes the list to its foot.
    await page.evaluate(async () => {
      window.__CONVERSATION__.status = "idle";
      window.__ANSWER__.wire_send = () => null;
      await pollHelperPages();
      const face = document.querySelector("#worker-view");
      const input = face.querySelector(".worker-composer-box");
      input.value = "다음 부탁";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      face.querySelector(".worker-composer").requestSubmit();
    });
    seen.afterSend = await still();
    // The one way back to the foot is the person's button (사용자 요청 09-24),
    // and at the foot it is not shown.
    seen.footDoor = await page.evaluate(() => {
      const doors = document.querySelector("#worker-view").querySelectorAll(".chat-foot-door, [class*='latest'], [class*='jump']");
      return { doors: doors.length, shown: doors[0]?.classList.contains("is-shown") ?? null };
    });
    ok(
      "A5: new words keep a list at its foot there; a wheel upward — even inside the last 50px — leaves at once and later words leave it where it stands; a wheel back to the foot comes back",
      seen.start.gap <= 1 && seen.wheeledUp.gap > 1 && seen.afterWheelUp.top === seen.wheeledUp.top &&
        seen.afterWheelUp.gap > 50 && seen.afterWheelDown.gap <= 1,
      JSON.stringify(seen),
    );
    ok(
      "A5: an upward key leaves before anything moved, End at the foot comes back, a thumb dragged up leaves, and a box inside the list that can still scroll keeps the wheel for itself",
      seen.afterKeyUp.top === seen.keyedUp.top && seen.afterKeyUp.gap > 50 && seen.afterEnd.gap <= 1 &&
        seen.afterDrag.top === seen.dragged.top && seen.afterDrag.gap > 50 &&
        JSON.stringify(seen.inner) === JSON.stringify({ up: true, down: true, outside: false }),
      JSON.stringify(seen),
    );
    ok(
      "A5: sending takes the list to its foot, and the one way back to it — the person's button (사용자 요청 09-24; the extension has none) — is not shown there",
      seen.afterSend.gap <= 1 && seen.footDoor.doors === 1 && seen.footDoor.shown === false,
      JSON.stringify(seen),
    );
    ok("A5: the scroll raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A5, the person's own ask (t-6824, 2026-09-24): 「지금 대화를 누르면 대화
 * 목록 맨 아래부터 시작하면 좋을 것 같아 — CC처럼 지금 보던 화면부터. 아니면
 * 위쪽에 있으면 맨 아래로 가기가 있음 좋겠어.」 A conversation opens at its
 * foot — also when its page was built before the list had a box (a leaf not
 * yet on screen), and it stays there when the list's box changes size. A
 * reader who left the foot gets a button back to it, gone again at the foot;
 * a conversation looked at again stands where its reader left it. The
 * extension has no such button; this page grows one because the person asked
 * for it. */
export async function testConversationFoot(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const history = (said) => {
      const rows = [];
      for (let at = 0; at < 40; at += 1) {
        rows.push({ role: "user", text: `${said} ${at}` });
        rows.push({ role: "assistant", text: `답 ${at}: ${"긴 문장이 여러 줄로 이어진다. ".repeat(6)}` });
      }
      return rows;
    };
    await page.evaluate(() => {
      let wire = 40;
      window.__NEXT_WIRE__ = () => {
        wire += 1;
        window.__ANSWER__.wire_start = (args) => ({ id: wire, agent: args.agent, protocol: "claude-stream", version: "2.1.280", model: null, session: null });
      };
      // The list on screen, its door, and where both stand once the list has
      // stopped moving (the door's glide included).
      window.__FOOT__ = {
        list: () => [...document.querySelectorAll(".helper-turns")].find((one) => one.checkVisibility()) ?? null,
        door: () => window.__FOOT__.list()?.parentElement.querySelector(":scope > .chat-foot-door") ?? null,
        async still() {
          const list = window.__FOOT__.list();
          let last = -1;
          let same = 0;
          for (let beat = 0; beat < 240 && same < 5; beat += 1) {
            await new Promise((done) => requestAnimationFrame(done));
            same = list.scrollTop === last ? same + 1 : 0;
            last = list.scrollTop;
          }
          const door = window.__FOOT__.door();
          return {
            top: Math.round(list.scrollTop),
            gap: Math.round(list.scrollHeight - list.scrollTop - list.clientHeight),
            door: door === null ? null : door.classList.contains("is-shown") && door.checkVisibility({ visibilityProperty: true }),
          };
        },
        // The row the reader is reading: the first one under the list's top
        // edge that is not a person's words stuck there.
        reading() {
          const list = window.__FOOT__.list();
          const top = list.getBoundingClientRect().top;
          const row = [...list.querySelectorAll(":scope > [data-turn]:not(.is-user)")]
            .find((one) => one.getBoundingClientRect().bottom > top);
          return row ? { turn: row.dataset.turn, offset: Math.round(row.getBoundingClientRect().top - top) } : null;
        },
      };
    });
    const still = () => page.evaluate(() => window.__FOOT__.still());
    const reading = () => page.evaluate(() => window.__FOOT__.reading());
    const wheelOver = async (dy) => {
      const box = await page.evaluate(() => {
        const rect = window.__FOOT__.list().getBoundingClientRect();
        return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 3 };
      });
      await page.mouse.move(box.x, box.y);
      await page.mouse.wheel(0, dy);
      return still();
    };
    let grown = 0;
    const grow = async () => {
      grown += 1;
      await page.evaluate(async (grown) => {
        window.__CONVERSATION__.live = [{ role: "assistant", text: `흐르는 ${"새로 온 말이 한 줄 더 붙는다. ".repeat(30 * grown)}` }];
        await pollHelperPages();
        for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      }, grown);
      return still();
    };
    const seen = {};

    // --- a_conversation_opens_at_its_foot --------------------------------
    // Built while its leaf had no box (the page is painted, then the leaf
    // shows it): the list has no height to go to the foot of until then.
    await page.evaluate(() => {
      window.__NEXT_WIRE__();
      const veil = document.createElement("style");
      veil.id = "foot-veil";
      veil.textContent = ".worker-view { display: none !important; }";
      document.head.append(veil);
    });
    await openConversation(page, history("가"), { status: "working", live: [{ role: "assistant", text: "흐르는" }] });
    seen.first = await page.evaluate(() => activeTabId);
    await page.evaluate(() => document.getElementById("foot-veil").remove());
    seen.openedUnseen = await still();
    // Another conversation, opened on screen.
    await page.evaluate(async (rows) => {
      window.__NEXT_WIRE__();
      await openWirePage("claude", "/tmp/zerocode-window-test", { history: rows });
      await pollHelperPages();
    }, history("나"));
    seen.second = await page.evaluate(() => activeTabId);
    seen.openedSecond = await still();
    // The window grows shorter: a list at its foot stays there.
    const size = page.viewportSize();
    await page.setViewportSize({ width: size.width, height: size.height - 240 });
    seen.shrunk = await still();
    await page.setViewportSize(size);
    seen.regrown = await still();
    ok(
      "a_conversation_opens_at_its_foot: a conversation opens at its foot — one whose page was built before its leaf was on screen too — and a list at its foot stays there when the window changes its height",
      seen.openedUnseen.gap <= 1 && seen.openedSecond.gap <= 1 && seen.shrunk.gap <= 1 && seen.regrown.gap <= 1,
      JSON.stringify(seen),
    );

    // --- a_reader_who_left_the_foot_gets_a_way_back_and_the_button_hides_at_the_foot
    await page.evaluate((id) => setActiveTab(id), seen.first);
    seen.backAtFoot = await still();
    seen.doorShape = await page.evaluate(() => {
      const door = window.__FOOT__.door();
      return door && { tag: door.tagName, type: door.type, label: door.getAttribute("aria-label"), words: door.textContent.trim() };
    });
    seen.left = await wheelOver(-900);
    seen.leftGrown = await grow();
    const doorAt = await page.evaluate(() => {
      const rect = window.__FOOT__.door()?.getBoundingClientRect();
      return rect ? { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 } : null;
    });
    if (doorAt) await page.mouse.click(doorAt.x, doorAt.y);
    seen.pressed = await still();
    seen.followsAgain = await grow();
    // The keyboard reaches it too: a button, focused, answers Enter.
    seen.leftAgain = await wheelOver(-900);
    await page.evaluate(() => window.__FOOT__.door()?.focus());
    await page.keyboard.press("Enter");
    seen.entered = await still();
    ok(
      "a_reader_who_left_the_foot_gets_a_way_back_and_the_button_hides_at_the_foot: at the foot the button is not shown; a reader who wheels up gets it — a button with its own name — and new words leave them where they stand with it shown; pressing it (the pointer, or Enter) takes the list to its foot, hides it and the words are followed again",
      seen.backAtFoot.gap <= 1 && seen.backAtFoot.door === false &&
        seen.doorShape?.tag === "BUTTON" && seen.doorShape.type === "button" && Boolean(seen.doorShape.label) &&
        seen.left.gap > 50 && seen.left.door === true &&
        seen.leftGrown.top === seen.left.top && seen.leftGrown.door === true &&
        seen.pressed.gap <= 1 && seen.pressed.door === false && seen.followsAgain.gap <= 1 && seen.followsAgain.door === false &&
        seen.leftAgain.door === true && seen.entered.gap <= 1 && seen.entered.door === false,
      JSON.stringify(seen),
    );

    // --- reopening_the_same_conversation_keeps_the_readers_place ----------
    seen.readerLeft = await wheelOver(-1500);
    seen.place = await reading();
    await page.evaluate((id) => setActiveTab(id), seen.second);
    seen.otherAtFoot = await still();
    await page.evaluate((id) => setActiveTab(id), seen.first);
    seen.returned = await still();
    seen.returnedPlace = await reading();
    // Looked away from and back to once more, it is still where it was left.
    await page.evaluate((id) => setActiveTab(id), seen.second);
    await page.evaluate((id) => setActiveTab(id), seen.first);
    seen.stillAway = await still();
    // A pane's own conversation turned to its terminal and back.
    await page.evaluate(async (turns) => {
      const term = await openTermTab({ placement: "tab" });
      paneAgents.set(term, "zo");
      hookStates.set(term, "idle");
      window.__FOOT_TERM__ = term;
      window.__ANSWER__.pane_log = () => ({ found: true, next: 1, turns, skipped: false, more: false, folded: false, model: "claude-opus-5" });
      await setPaneChat(term, true);
      await pollHelperPages();
      window.__ANSWER__.pane_log = () => ({ found: true, next: 1, turns: [], skipped: false, more: false, folded: false, model: "claude-opus-5" });
    }, history("다"));
    seen.paneOpened = await still();
    seen.paneLeft = await wheelOver(-1500);
    seen.panePlace = await reading();
    await page.evaluate(async () => {
      await setPaneChat(window.__FOOT_TERM__, false);
      for (let beat = 0; beat < 2; beat += 1) await window.__PAINTED__();
      await setPaneChat(window.__FOOT_TERM__, true);
      await pollHelperPages();
    });
    seen.paneReturned = await still();
    seen.paneReturnedPlace = await reading();
    const near = (a, b) => a !== null && b !== null && a.turn === b.turn && Math.abs(a.offset - b.offset) <= 2;
    ok(
      "reopening_the_same_conversation_keeps_the_readers_place: a conversation looked away from and back to stands at the row its reader left it on, the button shown — the other one, left at its foot, opens at its foot — and a pane's conversation turned to its terminal and back does the same",
      seen.readerLeft.gap > 50 && seen.otherAtFoot.gap <= 1 &&
        near(seen.place, seen.returnedPlace) && seen.returned.door === true &&
        seen.stillAway.top === seen.returned.top && seen.stillAway.door === true && seen.paneOpened.gap <= 1 &&
        seen.paneLeft.gap > 50 && near(seen.panePlace, seen.paneReturnedPlace) && seen.paneReturned.door === true,
      JSON.stringify(seen),
    );

    // --- the_button_names_itself_in_five_languages_without_a_title --------
    await page.evaluate((id) => setActiveTab(id), seen.first);
    seen.words = await page.evaluate(async () => {
      const words = {};
      for (const code of ["ko", "en", "ja", "zh", "es"]) {
        setLocale(code, { refresh: false, persist: false });
        paintWorkerView(tabs.find((tab) => tab.id === activeTabId));
        await window.__PAINTED__();
        const door = window.__FOOT__.door();
        words[code] = door && {
          label: door.getAttribute("aria-label"),
          said: door.textContent.trim(),
          titled: door.hasAttribute("title") || door.querySelector("[title]") !== null,
        };
      }
      setLocale("ko", { refresh: false, persist: false });
      return words;
    });
    const spoken = Object.values(seen.words);
    ok(
      "the_button_names_itself_in_five_languages_without_a_title: the button says its name in ko, en, ja, zh and es — five different sentences, the words it shows the same as the name it is read by — and carries no title tooltip",
      spoken.every((one) => one && one.label && one.said === one.label && !one.titled) &&
        new Set(spoken.map((one) => one?.label)).size === 5,
      JSON.stringify(seen.words),
    );
    ok("A5: the foot and its button raised no page errors", faults.length === 0, JSON.stringify(faults));
  } finally {
    await page.close();
  }
}

/* A6 — a helper at work. The extension's live helper rows (2.1.280 `E85`,
 * fed by `task_started` / `task_progress`): one row per local agent while it
 * runs — the description, and after a colon its summary or latest step
 * (`DU0`); its tokens and tools once there are tokens (`FU0`), and its time
 * (`MU0`): `12.8k tokens · 1 tool · 1m 3s`. Past four rows the first three
 * stand and one more sums the rest (`sD1`, `PU0`, `jU0`). The rows stand at
 * the list's foot over the status line, and leave with the helper. A pane's
 * helpers (`paneSubagents`) say their name and their tool count. */
export async function testConversationAgents(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, [
      { role: "user", text: "시작" },
      { role: "tool", text: "Agent · count md files", tool: { call_id: "toolu_1", name: "Agent", input: "{\"description\":\"count md files\"}", is_error: false } },
    ], { status: "working" });
    const seen = await page.evaluate(async () => {
      const seen = {};
      const face = document.querySelector("#worker-view");
      const settle = async () => {
        await pollHelperPages();
        for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      };
      const rows = () => [...face.querySelectorAll(".helper-agents > .helper-agent")].map((row) => [
        row.querySelector(".helper-agent-label")?.textContent ?? null,
        row.querySelector(".helper-agent-meta")?.textContent ?? null,
      ]);
      seen.noneAtFirst = face.querySelector(".helper-agents") === null;
      const now = Date.now();
      window.__CONVERSATION__.tasks = [{
        task: "a1", call: "toolu_1", description: "count md files", step: "Running Count .md files",
        tokens: 12842, tools: 1, started_ms: now - 62_700,
      }];
      await settle();
      seen.one = rows();
      const block = face.querySelector(".helper-agents");
      seen.atFoot = block?.nextElementSibling?.classList.contains("helper-status") === true;
      seen.dotBlinks = block ? getComputedStyle(block.querySelector(".helper-agent-dot")).animationName : null;
      window.__CONVERSATION__.tasks = [{ task: "a1", call: "toolu_1", description: "count md files", summary: "count md files", tokens: 0, tools: 0, started_ms: now - 4_200 }];
      await settle();
      seen.fresh = rows();
      window.__CONVERSATION__.tasks = ["a1", "a2", "a3", "a4", "a5"].map((task, at) => ({
        task, description: `helper ${at}`, tokens: at === 4 ? 0 : 1000 * (at + 1), tools: at, started_ms: now - 5_000,
      }));
      await settle();
      seen.five = rows();
      window.__CONVERSATION__.tasks = [];
      await settle();
      seen.gone = face.querySelector(".helper-agents") === null;
      // A pane's helpers, read through the same shape.
      paneSubagents.set(7, [
        { id: "h1", name: "code-reviewer", state: "running", tool_calls: 12 },
        { id: "h2", name: "done-helper", state: "done", tool_calls: 3 },
      ]);
      const paneTasks = typeof helperTasksOf === "function"
        ? helperTasksOf({ term: 7, helper: { id: PANE_LOG_ID } })
        : [];
      seen.pane = paneTasks.map((task) => [helperTaskLabel(task), helperTaskMeta(task, Date.now())]);
      seen.helperPage = typeof helperTasksOf === "function"
        ? helperTasksOf({ term: 7, helper: { id: "agent-1" } }).length
        : null;
      paneSubagents.delete(7);
      return seen;
    });
    ok(
      "A6: a helper at work stands at the list's foot over the status line with the extension's words — its description and latest step, then its tokens, tools and time — blinking the live dot, and a fresh one with no tokens says only its time",
      seen.noneAtFirst && seen.atFoot && seen.dotBlinks === "helper-live-pulse" &&
        seen.one.length === 1 && seen.one[0][0] === "count md files: Running Count .md files" &&
        /^12\.8k 토큰 · 도구 1회 · 1분 [34]초$/.test(seen.one[0][1]) &&
        seen.fresh.length === 1 && seen.fresh[0][0] === "count md files" && /^[45]초$/.test(seen.fresh[0][1]),
      JSON.stringify(seen),
    );
    ok(
      "A6: past four helpers the first three stand and one row sums the rest, and the rows leave with the helpers",
      JSON.stringify(seen.five.map(([label]) => label)) === JSON.stringify(["helper 0", "helper 1", "helper 2", "+2개 더"]) &&
        /^4\.0k 토큰 · 도구 7회 · 합계 1\d초$/.test(seen.five[3][1]) && seen.gone,
      JSON.stringify(seen),
    );
    ok(
      "A6: a pane's running helpers say their name and tool count, a finished one is gone, and a helper's own page lists none",
      JSON.stringify(seen.pane) === JSON.stringify([["code-reviewer", "도구 12회"]]) && seen.helperPage === 0,
      JSON.stringify(seen),
    );
    ok("A6: the helpers raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A7 — the todo list. The extension draws a todo call as its list (2.1.280
 * `qD1` / `PG0`) under the head 「Update Todos」: the items' content beside a
 * box — ticked when done, `✽` under way, empty waiting — a done item faded
 * and struck through, and no count, `activeForm` or result line. Every call
 * is its own row. In the Focus view the newest call that has not failed
 * stands out of its fold, out or back (`ew0`), and none stands when that
 * call emptied the list. */
export async function testConversationTodos(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const todos = (states) => JSON.stringify({
      todos: states.map((status, at) => ({ content: `할 일 ${at + 1}`, status, activeForm: `하는 중 ${at + 1}` })),
    }, null, 2);
    const call = (id, states) => ({ role: "tool", text: "TodoWrite", tool: { call_id: id, name: "TodoWrite", input: todos(states), is_error: false } });
    const result = (id) => ({ role: "tool_result", text: "Todos have been modified successfully.", tool: { call_id: id, name: "TodoWrite", input: "", is_error: false } });
    await openConversation(page, [
      { role: "user", text: "시작" },
      call("t1", ["in_progress", "pending", "pending"]),
      result("t1"),
      { role: "tool", text: "Read · a.rs", tool: { call_id: "r1", name: "Read", input: "a.rs", is_error: false } },
      { role: "tool_result", text: "fn a() {}", tool: { call_id: "r1", name: "Read", input: "", is_error: false } },
      call("t2", ["completed", "in_progress", "pending"]),
      result("t2"),
      { role: "tool", text: "Read · b.rs", tool: { call_id: "r2", name: "Read", input: "b.rs", is_error: false } },
      { role: "tool_result", text: "fn b() {}", tool: { call_id: "r2", name: "Read", input: "", is_error: false } },
      { role: "assistant", text: "진행 중입니다." },
    ]);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const call = (id, states) => ({
        role: "tool", text: "TodoWrite",
        tool: { call_id: id, name: "TodoWrite", input: JSON.stringify({ todos: states.map((status, at) => ({ content: `할 일 ${at + 1}`, status })) }), is_error: false },
      });
      const result = (id) => ({ role: "tool_result", text: "Todos have been modified successfully.", tool: { call_id: id, name: "TodoWrite", input: "", is_error: false } });
      const face = document.querySelector("#worker-view");
      const settle = async () => {
        await pollHelperPages();
        for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      };
      const todoRows = () => [...face.querySelectorAll(".helper-turn.is-todo")];
      const items = (row) => [...row.querySelectorAll(".helper-todo")].map((item) => {
        const box = item.querySelector("input.helper-todo-box");
        const content = item.querySelector(".helper-todo-content");
        const look = getComputedStyle(content);
        return {
          text: content.textContent,
          state: box.checked ? "checked" : box.indeterminate ? "mixed" : "empty",
          disabled: box.disabled,
          struck: look.textDecorationLine.includes("line-through"),
          mark: getComputedStyle(box, "::after").content,
        };
      });
      const rows = todoRows();
      seen.rows = rows.length;
      seen.second = rows[1] ? items(rows[1]) : null;
      seen.heads = rows.map((row) => row.querySelector(".helper-tool-name")?.textContent);
      seen.wantHead = t("worker.todoHead", "할 일 갱신");
      seen.named = rows.map((row) => row.getAttribute("aria-label"));
      seen.noActiveForm = !face.textContent.includes("하는 중");
      seen.noGeneric = rows.every((row) => row.querySelector(".helper-tool-body, .helper-tool-result") === null);
      seen.argEmpty = rows.every((row) => row.querySelector(".helper-tool-arg")?.textContent === "");
      // The Focus view: the newest list stands out of its fold.
      face.querySelector(".worker-focus").click();
      await settle();
      const shown = (row) => row && !row.hidden && row.getBoundingClientRect().height > 0;
      const standing = () => todoRows().map(shown);
      seen.focusFirst = shown(rows[0]);
      seen.focusLatest = shown(rows[1]);
      seen.readsFolded = [...face.querySelectorAll(".helper-turn.is-tool:not(.is-todo)")].every((row) => row.hidden);
      // A newer list takes its place, and the one before goes back in.
      const log = window.__CONVERSATION__;
      log.turns.push(call("t3", ["completed", "completed", "in_progress"]), result("t3"));
      await settle();
      seen.afterThird = standing();
      // A call still out stands already; one that failed gives way to the one
      // before it. (A row is dressed again while it is out — while the run
      // runs — so the turn is out for those two beats.)
      log.status = "working";
      log.turns.push(call("t4", ["completed", "completed", "completed"]));
      await settle();
      seen.whileOut = standing();
      seen.outIsLive = todoRows().at(-1)?.classList.contains("is-live");
      log.turns.push({ ...result("t4"), text: "InputValidationError", tool: { ...result("t4").tool, is_error: true } });
      await settle();
      log.status = "idle";
      await settle();
      seen.afterFailure = standing();
      // A call that empties the list is a head alone, and nothing stands.
      log.turns.push({ role: "tool", text: "TodoWrite", tool: { call_id: "t5", name: "TodoWrite", input: JSON.stringify({ todos: [] }), is_error: false } }, result("t5"));
      await settle();
      seen.emptied = standing();
      seen.emptyHead = todoRows().at(-1)?.querySelector(".helper-todos") === null;
      face.querySelector(".worker-focus").click();
      await settle();
      seen.allBack = todoRows().every(shown);
      return seen;
    });
    ok(
      "A7: a todo call draws its list under the extension's head — a disabled box ticked when done, mixed (`✽`) under way, empty waiting, a done item struck through — with no activeForm, no result line, no generic body and nothing beside the head, while the row is still named for its tool",
      seen?.rows === 2 && JSON.stringify(seen.second?.map((item) => [item.text, item.state, item.disabled, item.struck])) ===
        JSON.stringify([["할 일 1", "checked", true, true], ["할 일 2", "mixed", true, false], ["할 일 3", "empty", true, false]]) &&
        seen.second?.[0].mark === "\"✓\"" && seen.second?.[1].mark === "\"✽\"" &&
        seen.heads.every((head) => head === seen.wantHead) && seen.named.every((name) => name.includes("TodoWrite")) &&
        seen.noActiveForm && seen.noGeneric && seen.argEmpty,
      JSON.stringify(seen),
    );
    ok(
      "A7: in the Focus view the newest list stands out of its fold while the older ones and the reads stay folded — a call still out stands at once, a failed one gives way to the one before it, a call that emptied the list leaves none standing — and off again every list stands",
      seen?.focusFirst === false && seen.focusLatest === true && seen.readsFolded &&
        JSON.stringify(seen.afterThird) === JSON.stringify([false, false, true]) &&
        JSON.stringify(seen.whileOut) === JSON.stringify([false, false, false, true]) && seen.outIsLive === true &&
        JSON.stringify(seen.afterFailure) === JSON.stringify([false, false, true, false]) &&
        JSON.stringify(seen.emptied) === JSON.stringify([false, false, false, false, false]) && seen.emptyHead &&
        seen.allBack,
      JSON.stringify(seen),
    );
    ok("A7: the todo lists raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A8 — images. The extension draws an image a message carries as a pill
 * (2.1.280 `pp`): a 12px thumbnail, `image.<kind>`, and its size once
 * loaded — the person's above their words, a tool's with what it handed
 * back — and a press opens it whole (`previewOverlay`): the image within 90%
 * of the window, the focus on the close button, Esc or the ground to leave,
 * the focus back. This page asks for a picture only when its pill is in view,
 * and once. */
export async function testConversationImages(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.evaluate(({ png }) => {
      window.__IMAGE_ASKS__ = [];
      window.__ANSWER__.wire_image = (args) => {
        window.__IMAGE_ASKS__.push(args.at);
        return png;
      };
    }, { png: FIXTURE_PNG });
    const history = [
      { role: "user", text: "[Image #1] 이 화면을 봐", images: [{ media_type: "image/png", at: "wire:1" }] },
      { role: "user", text: "", images: [{ media_type: "image/jpeg", at: "wire:3" }] },
    ];
    for (let at = 0; at < 30; at += 1) {
      history.push({ role: "assistant", text: `답 ${at}: ${"긴 문장이 이어진다. ".repeat(8)}` });
    }
    history.push({ role: "tool", text: "mcp__computer-use__screenshot", tool: { call_id: "s1", name: "mcp__computer-use__screenshot", input: "{}", is_error: false } });
    history.push({ role: "tool_result", text: "screenshot taken", tool: { call_id: "s1", name: "mcp__computer-use__screenshot", input: "", is_error: false }, images: [{ media_type: "image/png", at: "wire:2" }] });
    await openConversation(page, history);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const face = document.querySelector("#worker-view");
      const list = face.querySelector(".helper-turns");
      const frames = async (n) => {
        for (let beat = 0; beat < n; beat += 1) await window.__PAINTED__();
      };
      await frames(6);
      const pills = [...face.querySelectorAll(".helper-image")];
      seen.pills = pills.length;
      const people = [...face.querySelectorAll(".helper-turn.is-user")];
      seen.personPillFirst = people[0]?.firstElementChild?.classList.contains("helper-images") ?? false;
      seen.onlyPicture = people[1] ? [people[1].querySelector(".helper-images") !== null, people[1].querySelector(".helper-said")?.hidden] : null;
      seen.names = pills.map((pill) => pill.querySelector(".helper-image-name")?.textContent);
      seen.toolPillAfterResult = face.querySelector(".helper-turn.is-tool .helper-tool-result + .helper-images") !== null;
      // A result that is a picture alone says it with the pill, not 「출력 없음」.
      window.__CONVERSATION__.turns.push(
        { role: "tool", text: "mcp__computer-use__zoom", tool: { call_id: "z1", name: "mcp__computer-use__zoom", input: "{}", is_error: false } },
        { role: "tool_result", text: "", tool: { call_id: "z1", name: "mcp__computer-use__zoom", input: "", is_error: false }, images: [{ media_type: "image/png", at: "wire:4" }] },
      );
      await pollHelperPages();
      await frames(2);
      const zoom = [...face.querySelectorAll(".helper-turn.is-tool")].at(-1);
      seen.pictureAlone = zoom ? [zoom.querySelector(".helper-tool-result") === null, zoom.querySelector(".helper-images .helper-image") !== null] : null;
      // At the foot: the tool's picture is in view, and so is the picture on
      // the person's row that stands stuck at the list's top (the sticky
      // header); the earlier person's row, stuck under it, is covered — in
      // view by geometry only — and not asked for.
      seen.askedAtFoot = [...window.__IMAGE_ASKS__].sort();
      const toolPill = face.querySelector(".helper-turn.is-tool .helper-image");
      const thumb = toolPill?.querySelector(".helper-image-thumb");
      for (let beat = 0; beat < 60 && !(thumb?.naturalWidth > 0); beat += 1) await frames(1);
      await frames(2);
      const rect = thumb?.getBoundingClientRect();
      seen.thumbBox = rect ? [rect.width, rect.height] : null;
      seen.pillHeight = toolPill?.getBoundingClientRect().height ?? null;
      seen.size = toolPill?.querySelector(".helper-image-size")?.textContent ?? null;
      // Up to the top: now the person's are asked for.
      list.scrollTop = 0;
      for (let beat = 0; beat < 60 && window.__IMAGE_ASKS__.length < 4; beat += 1) await frames(1);
      seen.askedAtTop = [...window.__IMAGE_ASKS__].sort();
      // A press opens the picture whole; Esc closes it and does not interrupt.
      window.__CALLS__ = [];
      const before = window.__ANSWER__.wire_interrupt;
      window.__ANSWER__.wire_interrupt = () => {
        window.__CALLS__.push("interrupt");
        return null;
      };
      const first = face.querySelector(".helper-turn.is-user .helper-image");
      first.focus();
      first.click();
      for (let beat = 0; beat < 30 && !document.querySelector(".chat-image-preview"); beat += 1) await frames(1);
      const ground = document.querySelector(".chat-image-preview");
      const picture = ground?.querySelector(".chat-image-preview-image");
      seen.dialog = ground?.querySelector("[role=dialog]")?.getAttribute("aria-modal") ?? null;
      seen.closeFocused = document.activeElement?.classList.contains("chat-image-preview-close") ?? false;
      for (let beat = 0; beat < 60 && !(picture?.naturalWidth > 0); beat += 1) await frames(1);
      const box = picture?.getBoundingClientRect();
      seen.withinWindow = box ? box.width <= innerWidth * 0.9 + 1 && box.height <= innerHeight * 0.9 + 1 : false;
      seen.groundFixed = ground ? getComputedStyle(ground).position : null;
      seen.groundOver = ground ? Number(getComputedStyle(ground).zIndex) > 0 : false;
      document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
      await frames(2);
      seen.closedByEsc = document.querySelector(".chat-image-preview") === null;
      seen.focusBack = document.activeElement === first;
      seen.noInterrupt = window.__CALLS__.length === 0;
      window.__ANSWER__.wire_interrupt = before;
      // The ground closes it too; the picture was asked for once.
      first.click();
      for (let beat = 0; beat < 30 && !document.querySelector(".chat-image-preview"); beat += 1) await frames(1);
      document.querySelector(".chat-image-preview")?.click();
      await frames(2);
      seen.closedByGround = document.querySelector(".chat-image-preview") === null;
      seen.askedOnce = window.__IMAGE_ASKS__.length;
      // A picture that cannot be read keeps its name.
      window.__ANSWER__.wire_image = () => {
        throw new Error("이 이미지는 더 이상 없습니다");
      };
      window.__CONVERSATION__.turns.push({ role: "user", text: "하나 더", images: [{ media_type: "image/png", at: "wire:9" }] });
      await pollHelperPages();
      list.scrollTop = list.scrollHeight;
      await frames(6);
      const gone = [...face.querySelectorAll(".helper-turn.is-user .helper-image")].at(-1);
      for (let beat = 0; beat < 30 && !gone?.classList.contains("is-missing"); beat += 1) await frames(1);
      seen.missingKeepsName = gone?.classList.contains("is-missing") && gone.querySelector(".helper-image-name")?.textContent === "image.png";
      return seen;
    });
    ok(
      "A8: an image stands as the extension's pill — a 12px thumbnail in a 24px pill, `image.<kind>` and its size once loaded — above the person's words (a picture sent alone stands with no empty bubble) and under the line a tool handed it back with",
      seen.pills === 3 && seen.personPillFirst && JSON.stringify(seen.onlyPicture) === JSON.stringify([true, true]) &&
        JSON.stringify(seen.names) === JSON.stringify(["image.png", "image.jpeg", "image.png"]) && seen.toolPillAfterResult &&
        JSON.stringify(seen.pictureAlone) === JSON.stringify([true, true]) &&
        JSON.stringify(seen.thumbBox) === JSON.stringify([12, 12]) && seen.pillHeight === 24 && seen.size === "1×1",
      JSON.stringify(seen),
    );
    ok(
      "A8: a picture is asked for only when its pill comes into view — a person's row stuck under a later person's sticky header is not in view — and only once; one that cannot be read keeps its name",
      JSON.stringify(seen.askedAtFoot) === JSON.stringify(["wire:2", "wire:3", "wire:4"]) &&
        JSON.stringify(seen.askedAtTop) === JSON.stringify(["wire:1", "wire:2", "wire:3", "wire:4"]) && seen.askedOnce === 4 &&
        seen.missingKeepsName,
      JSON.stringify(seen),
    );
    ok(
      "A8: a press opens the picture whole in a modal over the page within 90% of the window with the focus on its close; Esc closes it without interrupting the turn and gives the focus back, and so does a press on the ground",
      seen.dialog === "true" && seen.closeFocused && seen.withinWindow && seen.groundFixed === "fixed" && seen.groundOver &&
        seen.closedByEsc && seen.focusBack && seen.noInterrupt && seen.closedByGround,
      JSON.stringify(seen),
    );
    ok("A8: the images raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A9 — copying. The extension's copy button (2.1.280 `gN`) writes the text
 * and turns its icon to a check for 2 s, with no word and no new name: under
 * each answer (`Copy response`, the words the answer shows) and over each
 * code block's top right corner (`Copy code`, shown while the block is under
 * the pointer, copying the block as it stands). One clipboard door. */
export async function testConversationCopies(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, [
      { role: "user", text: "고쳐 줘" },
      { role: "assistant", text: "고쳤습니다.\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n그리고 하나 더:\n\n```sh\ncargo test\n```\n\n<system-reminder>숨은 줄</system-reminder>" },
    ]);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const face = document.querySelector("#worker-view");
      const answer = face.querySelector(".helper-turn.is-assistant");
      // The harness's clipboard records every write (`__CLIPBOARD_WRITES__`).
      const written = window.__CLIPBOARD_WRITES__;
      {
        const frames = [...answer.querySelectorAll(".helper-said .helper-code")];
        seen.frames = frames.length;
        const copies = frames.map((frame) => frame.querySelector(":scope > .helper-code-copy"));
        seen.names = copies.map((copy) => copy?.getAttribute("aria-label"));
        seen.want = t("worker.copyCode", "코드 복사");
        seen.hiddenAtRest = copies.every((copy) => getComputedStyle(copy).opacity === "0");
        const box = frames[0].getBoundingClientRect();
        const corner = copies[0].getBoundingClientRect();
        seen.corner = [Math.round(box.right - corner.right), Math.round(corner.top - box.top)];
        seen.inset = parseFloat(getComputedStyle(face).getPropertyValue("--chat-code-copy-inset"));
        seen.blockText = frames[0].querySelector("pre").textContent;
        copies[0].click();
        await window.__PAINTED__();
        seen.codeWritten = written.at(-1);
        seen.checked = copies[0].querySelector("use")?.getAttribute("href");
        seen.nameKept = copies[0].getAttribute("aria-label") === seen.want && copies[0].textContent.trim() === "";
        // The answer's copy: the words the answer shows, then the check.
        const copy = answer.querySelector(".helper-actions .helper-copy");
        copy.click();
        await window.__PAINTED__();
        seen.answerWritten = written.at(-1);
        seen.answerChecked = copy.querySelector("use")?.getAttribute("href");
        // Back to the copy icon after the panel's 2 s.
        await new Promise((done) => setTimeout(done, CHAT_COPIED_MS + 150));
        seen.backAfter = [copies[0].querySelector("use")?.getAttribute("href"), copy.querySelector("use")?.getAttribute("href")];
      }
      return seen;
    });
    ok(
      "A9: every code block in an answer wears the extension's copy over its top right corner, 4px in (a token), unseen until the block is under the pointer; a press writes the block as it stands and turns the icon to a check, the name unchanged",
      seen.frames === 2 && seen.names.every((name) => name === seen.want) && seen.hiddenAtRest &&
        JSON.stringify(seen.corner) === JSON.stringify([seen.inset, seen.inset]) && seen.inset === 4 &&
        seen.codeWritten === seen.blockText && seen.blockText.startsWith("fn main()") &&
        seen.checked === "#i-check" && seen.nameKept,
      JSON.stringify(seen),
    );
    ok(
      "A9: the answer's copy writes the words the answer shows — without the CLI's own plumbing — turns to a check, and both copies turn back after the panel's 2 s",
      typeof seen.answerWritten === "string" && seen.answerWritten.startsWith("고쳤습니다.") && !seen.answerWritten.includes("system-reminder") &&
        seen.answerChecked === "#i-check" && JSON.stringify(seen.backAfter) === JSON.stringify(["#i-copy", "#i-copy"]),
      JSON.stringify(seen),
    );
    ok("A9: the copies raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* B1 — a row far from view keeps its height, not its body. The 400-turn
 * page (B0's fixture) builds whole only the rows at its foot — the rest are
 * born without their bodies — and stands bodies (answers' prose, calls' diffs
 * and IN/OUT boxes, pictures) only within two screens of the view; a row that
 * leaves reach keeps the height it stood at. Scrolling brings bodies back
 * before they are seen and the reader's row never moves, even as rows born
 * without their bodies take them above it; once every row has stood whole,
 * the list is as tall at the top as back at the foot. A row the person
 * opened, or holds a selection in, keeps its body; a quiet poll still writes
 * nothing. */
export async function testConversationShelf(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openFixtureConversation(page, conversationFixture());
    const seen = await page.evaluate(async () => {
      const seen = {};
      const face = document.querySelector("#worker-view");
      const list = face.querySelector(".helper-turns");
      const frames = async (n) => {
        for (let beat = 0; beat < n; beat += 1) await window.__PAINTED__();
      };
      // The watcher answers after a frame is laid out: let it.
      await frames(4);
      await new Promise((done) => setTimeout(done, 50));
      const rows = () => [...list.querySelectorAll(":scope > .helper-turn")];
      const reach = () => {
        const view = list.getBoundingClientRect();
        const margin = view.height * CHAT_SHELF.screens;
        return (row) => {
          const box = row.getBoundingClientRect();
          return box.bottom >= view.top - margin && box.top <= view.bottom + margin;
        };
      };
      const bodyOf = (row) => row.querySelector(":scope > .helper-tool-diff, :scope > .helper-tool-body, :scope > .helper-images") !== null ||
        (row.classList.contains("is-assistant") && row.querySelector(":scope > .helper-said")?.childElementCount > 0);
      const judge = () => {
        const near = reach();
        let farBuilt = 0;
        let nearShelved = 0;
        let shelved = 0;
        let born = 0;
        let heightsKept = true;
        for (const row of rows()) {
          if (!shelvable(row)) continue;
          if (row.__shelved) {
            shelved += 1;
            if (row.__shelved.height === null) born += 1;
            else if (Math.abs(row.getBoundingClientRect().height - row.__shelved.height) > 0.5) heightsKept = false;
            if (bodyOf(row)) farBuilt += 1;
          }
          if (near(row) && row.__shelved) nearShelved += 1;
          if (!near(row) && !row.__shelved && bodyOf(row) && !row.classList.contains("is-live")) farBuilt += 1;
        }
        return { shelved, born, farBuilt, nearShelved, heightsKept };
      };
      seen.atFoot = judge();
      seen.elements = list.getElementsByTagName("*").length;
      seen.height = list.scrollHeight;
      // Up through the history, a screen at a time, to the very top: the row
      // at the view's top stays where the scroll put it — bodies come back, or
      // stand for the first time, before they are seen (a row born without
      // its body grows above the view, and the list is moved by the growth,
      // so the walk takes more than a screen's worth of steps).
      const moves = [];
      for (let step = 0; step < 400 && list.scrollTop > 0; step += 1) {
        const before = list.scrollTop;
        list.scrollTop = Math.max(0, before - list.clientHeight);
        await frames(2);
        const probe = document.elementFromPoint(list.getBoundingClientRect().left + 40, list.getBoundingClientRect().top + 20)?.closest(".helper-turn");
        const top = probe?.getBoundingClientRect().top ?? 0;
        await frames(2);
        moves.push(Math.abs((probe?.getBoundingClientRect().top ?? 0) - top));
        // What the person sees is never a row without its body.
        const view = list.getBoundingClientRect();
        const blank = rows().filter((row) => {
          if (!row.__shelved || row.hidden) return false;
          const box = row.getBoundingClientRect();
          return box.bottom > view.top && box.top < view.bottom;
        }).length;
        if (blank > 0) seen.blankSeen = (seen.blankSeen ?? 0) + 1;
      }
      seen.maxMove = Math.max(0, ...moves);
      seen.atTop = judge();
      seen.heightAtTop = list.scrollHeight;
      // Back down to the foot: the list is as tall as it stood.
      for (let step = 0; step < 400 && chatFootGap(list) > 1; step += 1) {
        list.scrollTop += list.clientHeight;
        await frames(2);
      }
      await frames(2);
      seen.backAtFoot = judge();
      seen.heightAtFoot = list.scrollHeight;
      list.scrollTop = 0;
      await frames(3);
      // A row the person opened keeps its body far from view.
      const bigDiff = rows().find((row) => row.querySelector(":scope > .helper-tool-diff .helper-expand"));
      bigDiff?.querySelector(".helper-expand")?.click();
      await frames(1);
      // A row holding the person's selection keeps its body too.
      const answer = rows().find((row) => row.classList.contains("is-assistant") && row !== bigDiff && reach()(row) && row.querySelector(".helper-said p"));
      const range = document.createRange();
      range.selectNodeContents(answer.querySelector(".helper-said p"));
      getSelection().removeAllRanges();
      getSelection().addRange(range);
      list.scrollTop = list.scrollHeight;
      await frames(4);
      await new Promise((done) => setTimeout(done, 50));
      seen.openedKept = bigDiff ? !bigDiff.__shelved && bigDiff.querySelector(":scope > .helper-tool-diff") !== null : null;
      seen.selectionKept = !answer.__shelved && answer.querySelector(".helper-said p") !== null;
      getSelection().removeAllRanges();
      // A row holding the keyboard's focus keeps its body too: the door the
      // person stands on is not taken from under them by a wheel.
      list.scrollTop = 0;
      await frames(4);
      await new Promise((done) => setTimeout(done, 50));
      const focused = rows().find((row) => row !== bigDiff && reach()(row) && row.querySelector(":scope > .helper-tool-body .helper-expand"));
      const door = focused?.querySelector(":scope > .helper-tool-body .helper-expand") ?? null;
      door?.focus();
      list.scrollTop = list.scrollHeight;
      await frames(4);
      await new Promise((done) => setTimeout(done, 50));
      seen.focusKept = door ? !focused.__shelved && document.activeElement === door : null;
      door?.blur();
      // A quiet poll writes nothing.
      const watch = new MutationObserver(() => {});
      watch.observe(list, { childList: true, subtree: true, characterData: true, attributes: true });
      window.__PERF_LIVE__ = [];
      await pollHelperPages();
      await pollHelperPages();
      seen.quiet = watch.takeRecords().length;
      watch.disconnect();
      return seen;
    });
    ok(
      "B1: a 400-turn page opens with only the rows within two screens of its foot standing their bodies — the rows beyond were born without theirs — and a row that gave its body up keeps the height it stood at",
      seen.atFoot.shelved > 250 && seen.atFoot.born > 250 && seen.atFoot.farBuilt === 0 && seen.atFoot.nearShelved === 0 && seen.atFoot.heightsKept,
      JSON.stringify(seen),
    );
    ok(
      "B1: scrolling up through the history, rows come back — or stand for the first time — before they are seen and the reader's row never moves; at the top the foot's rows have gone in turn, and once every row has stood the list is as tall back at the foot as at the top",
      seen.maxMove <= 1 && !seen.blankSeen && seen.atTop.farBuilt === 0 && seen.atTop.nearShelved === 0 && seen.atTop.heightsKept &&
        seen.atTop.born === 0 && Math.abs(seen.heightAtFoot - seen.heightAtTop) <= 2 &&
        seen.backAtFoot.farBuilt === 0 && seen.backAtFoot.nearShelved === 0,
      JSON.stringify(seen),
    );
    ok(
      "B1: a row whose door the person opened, a row holding the person's selection and a row holding the keyboard's focus keep their bodies far from view; a quiet poll writes nothing",
      seen.openedKept === true && seen.selectionKept && seen.focusKept === true && seen.quiet === 0,
      JSON.stringify(seen),
    );
    ok("B1: the shelf raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* B2 — a closed conversation leaves nothing behind. The page stands in the
 * leaf's one worker host; closing its tab takes the page — its rows, its
 * watchers, its dock's observer — out of that host, so the document is back
 * to the elements it had before the conversation opened, and the next
 * conversation in the leaf builds on a bare host. */
export async function testConversationRelease(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const before = await page.evaluate(() => document.getElementsByTagName("*").length);
    await openFixtureConversation(page, conversationFixture({ blocks: 5 }));
    const seen = await page.evaluate(async (before) => {
      const seen = { before };
      for (let beat = 0; beat < 4; beat += 1) await window.__PAINTED__();
      const tabOf = () => tabs.find((one) => one.worker?.helper?.id === WIRE_LOG_ID);
      const host = () => document.querySelector("#worker-view");
      seen.open = document.getElementsByTagName("*").length;
      seen.listOpen = host()?.querySelector(".helper-turns") !== null;
      closeTab(tabOf().id);
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      seen.closed = document.getElementsByTagName("*").length;
      seen.listGone = document.querySelectorAll(".helper-turns").length === 0;
      seen.pageForgotten = host()?.__helperPage == null;
      // The next conversation in the leaf stands on the bare host.
      window.__CONVERSATION__ = { status: "idle", live: [], turns: [] };
      window.__ANSWER__.wire_log = (args) => ({
        found: true, skipped: false, next: 0, turns: [], status: "idle", asks: [], live: [], agent: "claude",
        protocol: "claude-stream", models: [], modes: [], commands: [], version: "2.1.280", tasks: [],
      });
      await openWirePage("claude", "/tmp/zerocode-window-test", { history: [{ role: "user", text: "다시" }, { role: "assistant", text: "네." }] });
      await pollHelperPages();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      seen.reopened = host()?.querySelectorAll(".helper-turns > .helper-turn").length ?? 0;
      return seen;
    }, before);
    ok(
      "B2: closing a conversation's tab takes its page out of the leaf's host — no list is left standing and the document is back within a few dozen elements of where it stood before the conversation opened — and the next conversation opens on the bare host",
      seen.listOpen && seen.open - seen.before > 300 && seen.listGone && seen.pageForgotten &&
        seen.closed - seen.before < 60 && seen.reopened === 2,
      JSON.stringify(seen),
    );
    ok("B2: the release raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* Run by itself (`node ui/tests/conversation-parity.mjs [--engine webkit]
 * [name…]`): every check above in file order, or only those whose function
 * names contain one of the words given, in Chromium or in WebKit (the
 * installed window's engine). The window gate runs the same checks as its
 * `conversation-*` suites. */
if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const all = {
    testConversationFont, testConversationFolds, testConversationPaths, testConversationKeys, testConversationStatus,
    testConversationScroll, testConversationFoot, testConversationAgents, testConversationTodos, testConversationImages,
    testConversationCopies, testConversationShelf, testConversationRelease,
  };
  const argv = process.argv.slice(2);
  const at = argv.indexOf("--engine");
  const engine = at >= 0 ? argv.splice(at, 2)[1] : "chromium";
  const wanted = argv.map((word) => word.toLowerCase());
  const { files, origin } = await createWindowServer();
  const browser = await (engine === "webkit" ? webkitType() : chromium).launch({ headless: true });
  let failed = 0;
  try {
    for (const [name, check] of Object.entries(all)) {
      if (wanted.length && !wanted.some((word) => name.toLowerCase().includes(word))) continue;
      await check(browser, origin, (said, pass, details = "") => {
        console.log(`${pass ? "PASS" : "FAIL"} ${said}${!pass ? `\n  ${details}` : ""}`);
        if (!pass) failed += 1;
      });
    }
  } finally {
    await browser.close();
    files.close();
  }
  if (failed) process.exitCode = 1;
}
