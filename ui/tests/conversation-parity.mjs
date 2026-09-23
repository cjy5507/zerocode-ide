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
        status: held.status, asks: held.asks ?? [], live: held.live, agent: "claude", protocol: "claude-stream",
        models: [], mode: held.mode ?? null, modes: held.modes ?? [], commands: [], version: "2.1.280",
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
