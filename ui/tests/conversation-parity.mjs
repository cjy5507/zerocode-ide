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
      // A tool the window has no word for stands under its own name — all Latin.
      { role: "tool", text: "Lookup · ui/shell.js", tool: { call_id: "r1", name: "Lookup", kind: "Lookup", input: "ui/shell.js", is_error: false } },
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
        control.textContent = "Lookup";
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
          tool: getComputedStyle(document.querySelector("#worker-view .helper-step-kind")).fontFamily,
        };
      }, choice);
      const drawn = {
        prose: await drawnFaces(cdp, "#worker-view .helper-turn.is-assistant .helper-said p"),
        tool: await drawnFaces(cdp, "#worker-view .helper-step-kind"),
        user: await drawnFaces(cdp, "#worker-view .helper-turn.is-user .helper-said"),
      };
      // The Latin words stand in the platform's sans: the unknown tool's name
      // is all Latin, and the prose and the person's words carry Latin beside
      // Hangul.
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
      { role: "tool", text: "Bash · cargo test", tool: { call_id: "b1", name: "Bash", kind: "bash", input: "cargo test", is_error: false } },
      { role: "tool_result", text: lines(200, "test"), tool: { call_id: "b1", is_error: false } },
      // A word between keeps steps of one kind from folding into one row.
      { role: "assistant", text: "이어서 봅니다." },
      { role: "tool", text: "Bash · ls", tool: { call_id: "b2", name: "Bash", kind: "bash", input: "ls", is_error: false } },
      { role: "tool_result", text: "a.rs\nb.rs", tool: { call_id: "b2", is_error: false } },
      { role: "assistant", text: "고칩니다." },
      { role: "tool", text: "Edit · src/big.rs", tool: { call_id: "e1", name: "Edit", kind: "edit", input: "{}", is_error: false,
        edits: [{ path: "src/big.rs", lines: diff(120) }] } },
      { role: "tool_result", text: "updated", tool: { call_id: "e1", is_error: false } },
      { role: "assistant", text: "하나 더." },
      { role: "tool", text: "Edit · src/small.rs", tool: { call_id: "e2", name: "Edit", kind: "edit", input: "{}", is_error: false,
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
      // A long output: the step is one closed line; pressed, its body stands
      // and its OUT row stops at the clip and says it has more.
      const [bash, ls, big, small] = tools;
      const openRow = async (row) => {
        row.open = true;
        await window.__PAINTED__();
      };
      seen.oldFold = list.querySelectorAll("details.helper-tool-more").length;
      seen.closedFirst = tools.length === 4 &&
        tools.every((row) => row.tagName === "DETAILS" && row.open === false && row.querySelector(".helper-step-body") === null);
      await openRow(bash);
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
      await openRow(ls);
      const lsOut = ls.querySelector(".helper-tool-output");
      seen.shortOut = (lsOut === null || !lsOut.classList.contains("is-clipped")) && ls.querySelector(".helper-expand") === null;
      // A long diff: 200px, only the rows the clip shows are built, the door.
      await openRow(big);
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
      await openRow(small);
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
      "A1: a long tool output waits one press away — the step is one closed line, and opened its OUT row stops at the extension's 60px (a token) with 「더 보기」 and opens into its own scrolling well — while a short output stands whole with no door",
      seen.tokens.tool === 60 && seen.oldFold === 0 && seen.closedFirst && seen.bodyShown && seen.outClipped &&
        near(seen.outHeight, seen.tokens.tool) && seen.outDoor === seen.more && seen.outOpen && seen.shortOut,
      JSON.stringify(seen),
    );
    ok(
      "A1: in an opened step a long diff's box stops at the extension's 200px (a token) with only the rows that clip shows built, 「더 보기」 builds the rest and opens it into its own scrolling well, 「접기」 closes it back — a short diff stands whole — and a quiet poll writes nothing",
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
      { role: "tool", text: `Read · ${root}/src/app.rs`, tool: { call_id: "r1", name: "Read", kind: "read", is_error: false,
        input: JSON.stringify({ file_path: `${root}/src/app.rs`, offset: 11, limit: 50 }, null, 2),
        file: { path: `${root}/src/app.rs`, offset: 11, limit: 50 } } },
      { role: "tool_result", text: "    11→line 11", tool: { call_id: "r1", is_error: false } },
      { role: "tool", text: `Edit · ${root}/src/edit.rs`, tool: { call_id: "e1", name: "Edit", kind: "edit", is_error: false,
        input: "{}", file: { path: `${root}/src/edit.rs`, search: "let fixed = true;" },
        edits: [{ path: `${root}/src/edit.rs`, lines: [{ kind: "add", text: "let fixed = true;", old: null, new: null }] }] } },
      { role: "tool_result", text: "updated", tool: { call_id: "e1", is_error: false } },
      { role: "tool", text: "Grep · needle", tool: { call_id: "g1", name: "Grep", kind: "grep", is_error: false, input: "{}" } },
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
      // A step is one closed line: the file's door stands at the top of its
      // body, which the press that opens the row brings.
      seen.closedNoDoor = tools.every((row) => doorOf(row) === null);
      for (const row of tools) row.open = true;
      await window.__PAINTED__();
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
      "A2: a Read step's file is a door in its opened row that says the lines it read and opens the file tab at the first of them — the CLI's own count — an Edit step's opens where its new text stands, and a Grep step names no file and links nothing",
      seen.closedNoDoor && seen.readDoor && seen.readWhere === seen.wantReadWhere && seen.grepDoor &&
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
      { role: "tool", text: "Agent · count md files", tool: { call_id: "toolu_1", name: "Agent", kind: "task", input: "{\"description\":\"count md files\"}", is_error: false } },
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
    const call = (id, states) => ({ role: "tool", text: "TodoWrite", tool: { call_id: id, name: "TodoWrite", kind: "TodoWrite", input: todos(states), is_error: false } });
    const result = (id) => ({ role: "tool_result", text: "Todos have been modified successfully.", tool: { call_id: id, name: "TodoWrite", kind: "TodoWrite", input: "", is_error: false } });
    await openConversation(page, [
      { role: "user", text: "시작" },
      call("t1", ["in_progress", "pending", "pending"]),
      result("t1"),
      { role: "tool", text: "Read · a.rs", tool: { call_id: "r1", name: "Read", kind: "read", input: "a.rs", is_error: false } },
      { role: "tool_result", text: "fn a() {}", tool: { call_id: "r1", name: "Read", kind: "read", input: "", is_error: false } },
      call("t2", ["completed", "in_progress", "pending"]),
      result("t2"),
      { role: "tool", text: "Read · b.rs", tool: { call_id: "r2", name: "Read", kind: "read", input: "b.rs", is_error: false } },
      { role: "tool_result", text: "fn b() {}", tool: { call_id: "r2", name: "Read", kind: "read", input: "", is_error: false } },
      { role: "assistant", text: "진행 중입니다." },
    ]);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const call = (id, states) => ({
        role: "tool", text: "TodoWrite",
        tool: { call_id: id, name: "TodoWrite", kind: "TodoWrite", input: JSON.stringify({ todos: states.map((status, at) => ({ content: `할 일 ${at + 1}`, status })) }), is_error: false },
      });
      const result = (id) => ({ role: "tool_result", text: "Todos have been modified successfully.", tool: { call_id: id, name: "TodoWrite", kind: "TodoWrite", input: "", is_error: false } });
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
      // A todo call is one closed line — the head and how many are done — and
      // its list is one press away.
      seen.heads = rows.map((row) => row.querySelector(".helper-step-kind")?.textContent);
      seen.wantHead = t("worker.todoHead", "할 일 갱신");
      seen.tallies = rows.map((row) => row.querySelector(".helper-step-res")?.textContent);
      seen.wantTallies = [0, 1].map((done) => t("worker.stepTodoDone", "{{done}}/{{total}} 완료", { done, total: 3 }));
      seen.closedFirst = rows.every((row) => row.open === false && row.querySelector(".helper-todos") === null);
      for (const row of rows) row.open = true;
      await settle();
      seen.second = rows[1] ? items(rows[1]) : null;
      seen.named = rows.map((row) => row.getAttribute("aria-label"));
      seen.noActiveForm = !face.textContent.includes("하는 중");
      seen.noGeneric = rows.every((row) => row.querySelector(".helper-tool-body, .helper-tool-input, .helper-tool-output") === null);
      seen.argEmpty = rows.every((row) => row.querySelector(".helper-step-target")?.textContent === "");
      for (const row of rows) row.open = false;
      await settle();
      // The Focus view: the newest list stands out of its fold.
      face.querySelector(".worker-focus").click();
      await settle();
      const shown = (row) => row && !row.hidden && row.getBoundingClientRect().height > 0;
      const standing = () => todoRows().map(shown);
      seen.focusFirst = shown(rows[0]);
      seen.focusLatest = shown(rows[1]);
      seen.focusListStands = rows[1]?.open === true && rows[1].querySelector(".helper-todos") !== null && rows[0]?.open === false;
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
      log.turns.push({ role: "tool", text: "TodoWrite", tool: { call_id: "t5", name: "TodoWrite", kind: "TodoWrite", input: JSON.stringify({ todos: [] }), is_error: false } }, result("t5"));
      await settle();
      seen.emptied = standing();
      seen.emptyHead = todoRows().at(-1)?.querySelector(".helper-todos") === null;
      face.querySelector(".worker-focus").click();
      await settle();
      seen.allBack = todoRows().every(shown);
      return seen;
    });
    ok(
      "A7: a todo call is one closed line — the extension's head and how many items are done — that opens to its list: a disabled box ticked when done, mixed (`✽`) under way, empty waiting, a done item struck through — with no activeForm, no generic body and no target beside the head, while the row is still named for its tool",
      seen?.rows === 2 && seen.closedFirst && JSON.stringify(seen.tallies) === JSON.stringify(seen.wantTallies) &&
        JSON.stringify(seen.second?.map((item) => [item.text, item.state, item.disabled, item.struck])) ===
        JSON.stringify([["할 일 1", "checked", true, true], ["할 일 2", "mixed", true, false], ["할 일 3", "empty", true, false]]) &&
        seen.second?.[0].mark === "\"✓\"" && seen.second?.[1].mark === "\"✽\"" &&
        seen.heads.every((head) => head === seen.wantHead) && seen.named.every((name) => name.includes("TodoWrite")) &&
        seen.noActiveForm && seen.noGeneric && seen.argEmpty,
      JSON.stringify(seen),
    );
    ok(
      "A7: in the Focus view the newest list stands out of its fold, open, while the older ones and the reads stay folded — a call still out stands at once, a failed one gives way to the one before it, a call that emptied the list leaves none standing — and off again every list stands",
      seen?.focusFirst === false && seen.focusLatest === true && seen.focusListStands && seen.readsFolded &&
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
    history.push({ role: "tool", text: "mcp__computer-use__screenshot", tool: { call_id: "s1", name: "mcp__computer-use__screenshot", kind: "mcp__computer-use__screenshot", input: "{}", is_error: false } });
    history.push({ role: "tool_result", text: "screenshot taken", tool: { call_id: "s1", name: "mcp__computer-use__screenshot", kind: "mcp__computer-use__screenshot", input: "", is_error: false }, images: [{ media_type: "image/png", at: "wire:2" }] });
    await openConversation(page, history);
    const seen = await page.evaluate(async () => {
      const seen = {};
      const face = document.querySelector("#worker-view");
      const list = face.querySelector(".helper-turns");
      const frames = async (n) => {
        for (let beat = 0; beat < n; beat += 1) await window.__PAINTED__();
      };
      await frames(6);
      // A step is one closed line: the pictures it handed back come with the
      // press that opens its row.
      seen.toolClosed = face.querySelector(".helper-turn.is-tool .helper-image") === null;
      for (const row of face.querySelectorAll(".helper-turn.is-tool")) row.open = true;
      await frames(6);
      const pills = [...face.querySelectorAll(".helper-image")];
      seen.pills = pills.length;
      const people = [...face.querySelectorAll(".helper-turn.is-user")];
      seen.personPillFirst = people[0]?.firstElementChild?.classList.contains("helper-images") ?? false;
      seen.onlyPicture = people[1] ? [people[1].querySelector(".helper-images") !== null, people[1].querySelector(".helper-said")?.hidden] : null;
      seen.names = pills.map((pill) => pill.querySelector(".helper-image-name")?.textContent);
      seen.toolPillInBody = face.querySelector(".helper-turn.is-tool > .helper-step-body > .helper-images .helper-image") !== null;
      // A result that is a picture alone says it on the line and with the
      // pill, not 「출력 없음」.
      window.__CONVERSATION__.turns.push(
        { role: "tool", text: "mcp__computer-use__zoom", tool: { call_id: "z1", name: "mcp__computer-use__zoom", kind: "mcp__computer-use__zoom", input: "{}", is_error: false } },
        { role: "tool_result", text: "", tool: { call_id: "z1", name: "mcp__computer-use__zoom", kind: "mcp__computer-use__zoom", input: "", is_error: false }, images: [{ media_type: "image/png", at: "wire:4" }] },
      );
      await pollHelperPages();
      await frames(2);
      const zoom = [...face.querySelectorAll(".helper-turn.is-tool")].at(-1);
      const zoomLine = zoom?.querySelector(".helper-step-res")?.textContent;
      if (zoom) zoom.open = true;
      await frames(6);
      seen.pictureAlone = zoom
        ? [zoomLine === t("worker.stepImages", "이미지 {{n}}장", { n: 1 }) && !zoom.textContent.includes(t("worker.noOutput", "출력 없음")),
          zoom.querySelector(".helper-images .helper-image") !== null]
        : null;
      // A row opens where it stands, as a thought's does, so the body of the
      // last row grows below the fold: a person scrolls down to what they
      // pressed, and only then is its picture in view.
      list.scrollTop = list.scrollHeight;
      await frames(4);
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
      "A8: an image stands as the extension's pill — a 12px thumbnail in a 24px pill, `image.<kind>` and its size once loaded — above the person's words (a picture sent alone stands with no empty bubble) and in the opened row of the step that handed it back, which says so on its closed line",
      seen.toolClosed && seen.pills === 3 && seen.personPillFirst && JSON.stringify(seen.onlyPicture) === JSON.stringify([true, true]) &&
        JSON.stringify(seen.names) === JSON.stringify(["image.png", "image.jpeg", "image.png"]) && seen.toolPillInBody &&
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
 * born without their bodies — and stands bodies (answers' prose) only within
 * two screens of the view; a row that leaves reach keeps the height it stood
 * at. Scrolling brings bodies back before they are seen and the reader's row
 * never moves, even as rows born without their bodies take them above it;
 * once every row has stood whole, the list is as tall at the top as back at
 * the foot. A step is one closed line and has no body to give up: the one a
 * person opened keeps the body its press built. A row that holds a selection
 * or the keyboard's focus keeps its body; a quiet poll still writes
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
      const bodyOf = (row) => row.classList.contains("is-assistant") && row.querySelector(":scope > .helper-said")?.childElementCount > 0;
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
      // A step the person opened keeps the body its press built, far from view.
      const bigDiff = rows().find((row) => row.classList.contains("is-tool") && !row.classList.contains("is-run") && row.__turn?.tool?.edits?.length > 0);
      if (bigDiff) bigDiff.open = true;
      await frames(2);
      // A row holding the person's selection keeps its body too.
      const answer = rows().find((row) => row.classList.contains("is-assistant") && row !== bigDiff && reach()(row) && row.querySelector(".helper-said p"));
      const range = document.createRange();
      range.selectNodeContents(answer.querySelector(".helper-said p"));
      getSelection().removeAllRanges();
      getSelection().addRange(range);
      list.scrollTop = list.scrollHeight;
      await frames(4);
      await new Promise((done) => setTimeout(done, 50));
      seen.openedKept = bigDiff ? !bigDiff.__shelved && bigDiff.open && bigDiff.querySelector(":scope > .helper-step-body > .helper-tool-diff") !== null : null;
      seen.selectionKept = !answer.__shelved && answer.querySelector(".helper-said p") !== null;
      getSelection().removeAllRanges();
      // A row holding the keyboard's focus keeps its body too: the control the
      // person stands on is not taken from under them by a wheel.
      list.scrollTop = 0;
      await frames(4);
      await new Promise((done) => setTimeout(done, 50));
      const focused = rows().find((row) => row.classList.contains("is-assistant") && reach()(row) && row.querySelector(":scope > .helper-actions .helper-copy"));
      const door = focused?.querySelector(":scope > .helper-actions .helper-copy") ?? null;
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
      "B1: a 400-turn page opens with only the answers within two screens of its foot standing their prose — the answers beyond were born without theirs — and a row that gave its body up keeps the height it stood at",
      seen.atFoot.shelved > 60 && seen.atFoot.born > 60 && seen.atFoot.farBuilt === 0 && seen.atFoot.nearShelved === 0 && seen.atFoot.heightsKept,
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
      "B1: a step the person opened, an answer holding the person's selection and an answer holding the keyboard's focus keep their bodies far from view; a quiet poll writes nothing",
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

/* C — the steps (t-15682). What an agent did stands in the page as steps a
 * person can take in at a glance and open when they want the rest: a step is
 * one closed line — what it was, what it touched, what came of it, how long —
 * whose raw input and output are one press away, opening in place and staying
 * open while the page updates; steps of one kind in a row are one row that
 * opens to its members, a failed one never among them; a thought is one line
 * (its heading, or its newest sentence) that opens in place; while the turn
 * is out the foot line says the step that is out; a finished helper's report
 * stands on top of its page with the steps folded under it. Pane page, helper
 * page and wire page are one painter, so each of these is read on all three.
 * Everything here is synthetic — a shop app's profile screen, never a real
 * path or address. The claims are read off the page a browser laid out. */
const SHOP = "/Users/dev/shop-app";
const PROFILE = `${SHOP}/src/screens/profile`;
const CLOCK = 1_790_000_000_000;
const READ_FIRST = ["Gallery.tsx", "GalleryGrid.tsx", "Avatar.tsx", "Header.tsx", "useProfile.ts", "styles.ts"];
const READ_LATER = ["Cell.tsx", "keys.ts"];
const BASH_COMMAND = 'rg -n "SCREEN_WIDTH" src';
const BASH_OUTPUT = Array.from({ length: 8 }, (_, at) => (at === 1
  ? `${PROFILE}/Header.tsx:21:// MARK_TWO_BASH`
  : `${PROFILE}/${READ_FIRST[at % READ_FIRST.length]}:${11 + at}:const SCREEN_WIDTH = Dimensions.get("window").width;`)).join("\n");
const WEB_URL = `https://example.com/products?${"filter=new&".repeat(20)}sort=price`;
const WEB_OUTPUT = "200 OK\ncontent-type: text/html\n\n<title>Products</title>\n<h1>Products</h1>";
const REPORT_WORDS = ["원인은 셀의 key가 인덱스여서, 스크롤할 때마다 모든 셀이 다시 만들어지는 것이었습니다.", "key를 id로 바꿔 고쳤습니다."];

const fileBody = (name) => [
  "     1→import React from \"react\";",
  `     2→// MARK_TWO ${name}`,
  `     3→export const ${name.split(".")[0]} = () => null;`,
  "     4→",
  `     5→export default ${name.split(".")[0]};`,
].join("\n");

/* The brief, a thought with a heading, six reads, one that fails, two more, a
 * thought with none, a search, a page fetched, an edit and the answer — every
 * turn stamped, so how long a thought lasted (12 s, then 4 s) and how long a
 * call took are the file's own. */
function stepsFixture() {
  const turns = [];
  let clock = CLOCK;
  const say = (role, text, gap) => turns.push({ role, text, at_ms: (clock += gap) });
  const step = (id, name, kind, target, answer, { gap = 400, took = 300, failed = false, edits = null } = {}) => {
    turns.push({
      role: "tool", text: `${name} · ${target}`, at_ms: (clock += gap),
      tool: { call_id: id, name, kind, input: target, is_error: false, ...(edits && { edits }) },
    });
    turns.push({ role: "tool_result", text: answer, at_ms: (clock += took), tool: { call_id: id, is_error: failed } });
  };
  turns.push({ role: "user", text: "프로필 화면이 느린 이유를 찾아서 고쳐 줘.", at_ms: clock });
  say("thinking", "**Where the profile screen spends its time**\n\nThe gallery grid re-renders on every scroll tick. I should read the grid before I touch anything.", 500);
  READ_FIRST.forEach((name, at) => step(`r${at + 1}`, "Read", "read", `${PROFILE}/${name}`, fileBody(name), { gap: at === 0 ? 12000 : 400 }));
  step("r7", "Read", "read", `${PROFILE}/Missing.tsx`, `ENOENT: no such file or directory, open '${PROFILE}/Missing.tsx'`, { took: 200, failed: true });
  READ_LATER.forEach((name, at) => step(`r${at + 8}`, "Read", "read", `${PROFILE}/${name}`, fileBody(name)));
  say("thinking", "The list keys are indexes. That makes every cell remount. Memoizing the cells and keying by id should fix it.", 500);
  step("b1", "Bash", "bash", BASH_COMMAND, BASH_OUTPUT, { gap: 4000, took: 900 });
  step("w1", "WebFetch", "web", WEB_URL, WEB_OUTPUT, { took: 3900 });
  step("e1", "Edit", "edit", `${PROFILE}/Gallery.tsx`, `The file ${PROFILE}/Gallery.tsx has been updated.`, {
    edits: [{ path: `${PROFILE}/Gallery.tsx`, lines: [
      { kind: "del", text: "  key={index}", old: null, new: null },
      { kind: "add", text: "  key={item.id}", old: null, new: null },
    ] }],
  });
  say("assistant", REPORT_WORDS.join(" "), 500);
  return turns;
}

/* What every case below asks of the page, put on it once: the rows, the row's
 * own line (a step's `summary`; the old page's call line), the words a person
 * could read (a closed row's body is not among them) and a frame to settle. */
async function installStepsProbe(page) {
  await page.evaluate(() => {
    const texts = (root) => {
      const found = [];
      if (!root) return found;
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node; node = walker.nextNode()) found.push(node);
      return found;
    };
    // The wire's page and a helper's stand in `#worker-view`; a pane's conversation
    // stands in its own slot (`.pane-chat`).
    const list = () => document.querySelector("#worker-view .helper-turns") ?? document.querySelector(".pane-chat .helper-turns");
    const rows = () => [...(list()?.querySelectorAll(":scope > .helper-turn") ?? [])];
    window.__STEPS__ = {
      list,
      rows,
      steps: () => rows().filter((row) => row.classList.contains("is-tool")),
      thoughts: () => rows().filter((row) => row.classList.contains("is-thinking")),
      lineOf: (row) => row?.querySelector(":scope > summary") ?? row?.querySelector(":scope > .helper-tool-call") ?? row,
      holding: (root, needle) => texts(root).filter((node) => node.nodeValue.includes(needle)).map((node) => node.parentElement),
      shown: (root, needle) => texts(root).some((node) => node.nodeValue.includes(needle) &&
        node.parentElement.checkVisibility({ checkVisibilityCSS: true })),
      wordsOf: (node) => (node?.innerText ?? "").replace(/\s+/g, " ").trim(),
      tall: (node) => Math.round(node.getBoundingClientRect().height),
      settle: async () => {
        await window.__PAINTED__();
        await window.__PAINTED__();
      },
    };
  });
}

/* A helper's page, opened the way the sidebar opens it: its transcript read
 * from the parent's record, `state` the helper's own, `agent` the parent's. */
async function openHelperConversation(page, turns, state, { agent = "claude" } = {}) {
  await page.evaluate(async ({ turns, state, agent }) => {
    const term = await openTermTab({ placement: "tab" });
    const owner = tabOfTerm(term);
    paneAgents.set(term, agent);
    hookStates.set(term, "working");
    window.__HELPER_TURNS__ = turns;
    window.__ANSWER__.subagent_log = (args) => ({ found: true, next: turns.length, turns: turns.slice(args.after ?? 0) });
    await openHelperPage({ term, tab: owner, worktree: owner.worktree, agent }, { id: "steps-helper", name: "프로필 도우미", state });
    await pollHelperPages();
    await window.__PAINTED__();
  }, { turns, state, agent });
  await page.waitForSelector("#worker-view .helper-turns .helper-turn:not(.is-briefing)", { state: "attached" });
}

/* zo's row as the core catalog voices it (`agent_voice`, `zo`): the harness's own
 * list has none, and zo is the agent whose word for a turn that is out is a
 * static English one — no spinner verbs to turn through — which the Korean page
 * must not say aloud. */
export const ZO_ROW = {
  id: "zo", name: "ZO", favicon_domain: "", homepage_url: "", installed: true, found_as: "zo",
  unsupported_here: false, missing_requirement: null, takes_a_paste: true, ready: "quiet",
  glyph: "◐", glyph_cycle: [], busy_word: "Working…", spinner_verbs: [], todo_tool: "TodoWrite",
  helper_stop: "helper.stop",
};

/* The window's agent list with zo in it, before any page is opened on a zo. */
export async function standZo(page) {
  await page.evaluate(async (row) => {
    const listed = await invoke("list_agents", {});
    window.__ANSWER__.list_agents = () => [row, ...listed];
    agentMarks.clear();
    await refreshAgents();
  }, ZO_ROW);
}

/* A pane whose agent has no wire, its conversation view open on `turns`. */
async function openPaneConversation(page, turns) {
  await page.evaluate(async (turns) => {
    const term = await openTermTab({ placement: "tab" });
    paneAgents.set(term, "zo");
    hookStates.set(term, "working");
    window.__ANSWER__.pane_log = (args) => ({
      found: true, next: turns.length, turns: args.after == null ? turns : turns.slice(args.after),
      skipped: false, more: false, folded: false, model: "claude-opus-5",
    });
    await setPaneChat(term, true);
    await pollHelperPages();
    await window.__PAINTED__();
  }, turns);
  await page.waitForSelector(".pane-chat .helper-turns .helper-turn");
}

/* What the engine's accessibility tree says of the node a selector names. */
async function axOf(cdp, selector) {
  const doc = await cdp.send("DOM.getDocument", { depth: 0 });
  const found = await cdp.send("DOM.querySelector", { nodeId: doc.root.nodeId, selector });
  if (!found.nodeId) return null;
  const { nodes } = await cdp.send("Accessibility.getPartialAXTree", { nodeId: found.nodeId, fetchRelatives: false });
  const node = nodes[0];
  const prop = (name) => node?.properties?.find((one) => one.name === name)?.value?.value;
  return { role: node?.role?.value ?? null, name: node?.name?.value ?? "", expanded: prop("expanded") };
}

export async function testConversationSteps(browser, origin, ok) {
  await stepsOnTheWire(browser, origin, ok);
  await stepsLive(browser, origin, ok);
  await stepsStill(browser, origin, ok);
  await stepsReport(browser, origin, ok);
  await stepsByKeyboard(browser, origin, ok);
  await stepsAcrossPages(browser, origin, ok);
  await stepsByKind(browser, origin, ok);
  await stepsThoughtLine(browser, origin, ok);
  await stepsThoughtHandOver(browser, origin, ok);
  await stepsThoughtWrites(browser, origin, ok);
  await stepsFocusKeepsOpen(browser, origin, ok);
  await stepsSpaceOnALine(browser, origin, ok);
  await stepsSpaceBesideTheGraph(browser, origin, ok);
  await stepsStillPage(browser, origin, ok);
  await stepsShellTitle(browser, origin, ok);
  await stepsUnknownTimes(browser, origin, ok);
  await stepsHelperClock(browser, origin, ok);
  await stepsNowWithoutARow(browser, origin, ok);
  await stepsListUnderASentence(browser, origin, ok);
}

/* C1–C4, C9 — the rows the fixture comes to, closed, then pressed. */
async function stepsOnTheWire(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, stepsFixture());
    await installStepsProbe(page);
    const shape = await page.evaluate(async ({ first, later }) => {
      const { steps, thoughts, lineOf, wordsOf, tall, holding, shown, settle } = window.__STEPS__;
      const seen = {};
      await settle();
      const standing = steps();
      seen.stepCount = standing.length;
      seen.tags = standing.map((row) => `${row.tagName}${row.open ? "+open" : ""}`);
      seen.heights = standing.map(tall);
      seen.parts = standing.map((row) => row.getElementsByTagName("*").length);
      const bash = standing.find((row) => wordsOf(lineOf(row)).includes("SCREEN_WIDTH"));
      const web = standing.find((row) => wordsOf(lineOf(row)).includes("example.com/products"));
      seen.bashLine = wordsOf(lineOf(bash));
      seen.webLine = wordsOf(lineOf(web));
      seen.sameHeight = bash !== undefined && web !== undefined && Math.abs(tall(bash) - tall(web)) <= 1;
      // Six reads in a row are one row; the read that failed stands apart, in
      // the failure ink; the two after it are a row of their own.
      const [run, failed, tail] = standing;
      seen.runTag = `${run?.tagName}:${run?.classList.contains("is-run")}`;
      seen.runLine = wordsOf(lineOf(run));
      seen.runHides = first.slice(1).every((name) => !shown(run, name));
      seen.failedApart = failed?.classList.contains("is-failed") === true && !failed.classList.contains("is-run");
      seen.failedLine = wordsOf(lineOf(failed));
      const probe = document.createElement("i");
      probe.style.color = "var(--signal-halt-ink)";
      document.querySelector("#worker-view").append(probe);
      const halt = getComputedStyle(probe).color;
      probe.remove();
      const says = holding(lineOf(failed), "ENOENT")[0];
      seen.failedInk = says ? getComputedStyle(says).color === halt : null;
      seen.tailTag = `${tail?.tagName}:${tail?.classList.contains("is-run")}`;
      seen.tailLine = wordsOf(lineOf(tail));
      seen.tailHides = !shown(tail, later[1]);
      const thinking = thoughts();
      seen.thoughtCount = thinking.length;
      seen.thoughtTags = thinking.map((row) => `${row.tagName}${row.open ? "+open" : ""}:${tall(row)}`);
      seen.thoughtLines = thinking.map((row) => wordsOf(lineOf(row)));
      return seen;
    }, { first: READ_FIRST, later: READ_LATER });
    ok(
      "C1: a step is one closed line — each row the fixture's steps come to is a closed details no taller than one line of the extension's row (44px), as tall for a 260-character address as for a short command, with a handful of elements behind it, no body built for every row",
      shape.stepCount === 6 && shape.tags.every((tag) => tag === "DETAILS") && shape.heights.every((height) => height <= 44) &&
        shape.parts.every((count) => count <= 12) && shape.sameHeight,
      JSON.stringify(shape),
    );
    ok(
      "C1: a closed step says what it did in its own line — the command it ran, how many lines it printed, how long it took by the file's own stamps; a page fetched says its address and its time",
      shape.bashLine.includes(BASH_COMMAND) && /\b8\b/.test(shape.bashLine) && shape.bashLine.includes("0.9") &&
        shape.webLine.includes("example.com/products") && shape.webLine.includes("3.9"),
      JSON.stringify(shape),
    );
    ok(
      "C3: six reads in a row are one row — its line says six, names the first file and that five more stand behind it, and shows none of the other five until it is opened; the two reads after the failed one are one row of their own",
      shape.stepCount === 6 && shape.runTag === "DETAILS:true" && /\b6\b/.test(shape.runLine) && /\b5\b/.test(shape.runLine) &&
        shape.runLine.includes(READ_FIRST[0]) && shape.runHides &&
        shape.tailTag === "DETAILS:true" && /\b2\b/.test(shape.tailLine) && shape.tailLine.includes(READ_LATER[0]) && shape.tailHides,
      JSON.stringify(shape),
    );
    ok(
      "C3: a failed step is never folded into a row of its kind — it stands on its own between the two, says how it failed in its line (the error's first words), and wears the failure ink",
      shape.failedApart && shape.failedLine.includes("ENOENT") && shape.failedLine.includes("Missing.tsx") && shape.failedInk === true,
      JSON.stringify(shape),
    );
    ok(
      "C4: a thought is one closed line — the heading the model wrote, or its newest sentence when it wrote none (not the first), and how long it lasted by the file's own stamps",
      shape.thoughtCount === 2 && shape.thoughtTags.every((tag) => /^DETAILS:\d+$/.test(tag) && Number(tag.split(":")[1]) <= 44) &&
        shape.thoughtLines[0].includes("Where the profile screen spends its time") && !shape.thoughtLines[0].includes("re-renders") &&
        /\b12\b/.test(shape.thoughtLines[0]) &&
        shape.thoughtLines[1].includes("Memoizing the cells and keying by id should fix it") &&
        !shape.thoughtLines[1].includes("The list keys are indexes") && /\b4\b/.test(shape.thoughtLines[1]),
      JSON.stringify(shape),
    );

    const opened = await page.evaluate(async () => {
      const { list, rows, steps, lineOf, wordsOf, tall, shown, settle } = window.__STEPS__;
      const seen = {};
      const bash = steps().find((row) => wordsOf(lineOf(row)).includes("SCREEN_WIDTH"));
      if (!bash) return { missing: true, copies: 0, written: [] };
      const at = rows().indexOf(bash);
      const before = { top: bash.offsetTop, height: tall(bash), next: rows()[at + 1]?.offsetTop };
      seen.rawHidden = !shown(bash, "MARK_TWO_BASH");
      lineOf(bash).click();
      await settle();
      const after = { top: bash.offsetTop, height: tall(bash), next: rows()[at + 1]?.offsetTop };
      seen.opened = bash.open === true;
      seen.rawShown = shown(bash, "MARK_TWO_BASH");
      seen.inPlace = rows()[at] === bash && after.top === before.top && after.height > before.height &&
        Math.abs((after.next - before.next) - (after.height - before.height)) <= 1;
      // The words are the person's to copy: the command and what it printed,
      // each by its own button.
      const copies = [...bash.querySelectorAll("button")].filter((button) => !button.closest("summary"));
      window.__CLIPBOARD_WRITES__.length = 0;
      for (const button of copies) {
        button.click();
        await window.__PAINTED__();
      }
      seen.copies = copies.length;
      seen.written = [...window.__CLIPBOARD_WRITES__];
      // A poll that brings a new row leaves the open one open — the same
      // node, in the same place — and one that brings nothing writes nothing.
      window.__CONVERSATION__.turns.push({ role: "assistant", text: "다음으로 넘어갑니다.", at_ms: 1_790_000_060_000 });
      await pollHelperPages();
      await settle();
      seen.kept = bash.isConnected && bash.open === true && rows().indexOf(bash) === at && shown(bash, "MARK_TWO_BASH");
      seen.rowsAfter = rows().length;
      const watch = new MutationObserver(() => {});
      watch.observe(list(), { childList: true, subtree: true, characterData: true, attributes: true });
      await pollHelperPages();
      await settle();
      seen.quiet = watch.takeRecords().length;
      watch.disconnect();
      lineOf(bash).click();
      await settle();
      seen.closedAgain = bash.open === false && tall(bash) === before.height;
      return seen;
    });
    ok(
      "C2: the raw words are one press away — a closed step keeps its input and output out of sight, pressing its line opens the row in place (the same row, the same top, the rows below moved down by what it grew), and the command and what it printed each have their own copy",
      opened.rawHidden && opened.opened && opened.rawShown && opened.inPlace && opened.copies >= 2 &&
        opened.written.includes(BASH_COMMAND) && opened.written.includes(BASH_OUTPUT),
      JSON.stringify(opened),
    );
    ok(
      "C2: what was opened stays open while the page updates — a poll that brings a new row leaves the open one open, in its place — a poll that brings nothing writes nothing, and pressing again closes it back to the line it was",
      opened.kept && opened.rowsAfter === 11 && opened.quiet === 0 && opened.closedAgain,
      JSON.stringify(opened),
    );

    const members = await page.evaluate(async ({ first }) => {
      const { steps, lineOf, wordsOf, tall, shown, settle } = window.__STEPS__;
      const seen = {};
      const [run] = steps();
      lineOf(run)?.click();
      await settle();
      seen.open = run?.open === true;
      const inside = [...(run?.querySelectorAll("details") ?? [])];
      seen.members = inside.length;
      seen.named = inside.map((row) => first.some((name) => wordsOf(lineOf(row)).includes(name)));
      seen.inOrder = inside.every((row, at) => wordsOf(lineOf(row)).includes(first[at]));
      seen.memberTags = inside.map((row) => `${row.tagName}${row.open ? "+open" : ""}:${tall(row)}`);
      lineOf(run)?.click();
      await settle();
      seen.closedAgain = run?.open === false && first.slice(1).every((name) => !shown(run, name));
      return seen;
    }, { first: READ_FIRST });
    ok(
      "C3: the row of six opens to its six members, in the order they were read, each one closed line of its own, and closes back to its line",
      members.open && members.members === 6 && members.inOrder && members.memberTags.every((tag) => /^DETAILS:\d+$/.test(tag) && Number(tag.split(":")[1]) <= 44) && members.closedAgain,
      JSON.stringify(members),
    );

    const thought = await page.evaluate(async () => {
      const { thoughts, lineOf, shown, settle } = window.__STEPS__;
      const [, second] = thoughts();
      const top = second?.offsetTop;
      const seen = { hiddenClosed: !shown(second, "That makes every cell remount") };
      lineOf(second)?.click();
      await settle();
      seen.opens = second?.open === true && shown(second, "That makes every cell remount") && second.offsetTop === top;
      lineOf(second)?.click();
      await settle();
      seen.closes = second?.open === false;
      return seen;
    });
    ok(
      "C4: everything a thought thought stays out of sight until its line is pressed, which opens it in place, and pressing again closes it",
      thought.hiddenClosed && thought.opens && thought.closes,
      JSON.stringify(thought),
    );

    const inks = await page.evaluate(async () => {
      const { steps, thoughts, lineOf, settle } = window.__STEPS__;
      const canvas = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
      const rgba = (css) => {
        canvas.clearRect(0, 0, 1, 1);
        canvas.fillStyle = "#000000";
        canvas.fillStyle = css;
        canvas.fillRect(0, 0, 1, 1);
        const [red, green, blue, alpha] = canvas.getImageData(0, 0, 1, 1).data;
        return [red, green, blue, alpha / 255];
      };
      const mix = (top, under) => [0, 1, 2].map((at) => top[at] * top[3] + under[at] * (1 - top[3]));
      const backdrop = (node) => {
        const layers = [];
        for (let at = node; at; at = at.parentElement) {
          const paint = rgba(getComputedStyle(at).backgroundColor);
          if (paint[3] > 0) layers.push(paint);
          if (paint[3] === 1) break;
        }
        return layers.reduceRight((under, layer) => mix(layer, under), [255, 255, 255]);
      };
      const luminance = (colour) => colour.map((value) => {
        const unit = value / 255;
        return unit <= 0.03928 ? unit / 12.92 : ((unit + 0.055) / 1.055) ** 2.4;
      }).reduce((sum, unit, at) => sum + unit * [0.2126, 0.7152, 0.0722][at], 0);
      const ratio = (left, right) => {
        const [high, low] = [luminance(left), luminance(right)].sort((one, other) => other - one);
        return (high + 0.05) / (low + 0.05);
      };
      const measure = () => {
        const worst = { ratio: Infinity, of: "" };
        let checked = 0;
        for (const line of [...steps(), ...thoughts()].map(lineOf)) {
          for (const node of [line, ...line.querySelectorAll("*")]) {
            if (![...node.childNodes].some((child) => child.nodeType === 3 && child.nodeValue.trim() !== "")) continue;
            if (!node.checkVisibility({ checkVisibilityCSS: true })) continue;
            const under = backdrop(node);
            let opacity = 1;
            for (let at = node; at; at = at.parentElement) opacity *= parseFloat(getComputedStyle(at).opacity);
            const ink = rgba(getComputedStyle(node).color);
            const seen = ratio(mix([ink[0], ink[1], ink[2], ink[3] * opacity], under), under);
            checked += 1;
            if (seen < worst.ratio) {
              worst.ratio = Math.round(seen * 100) / 100;
              worst.of = `${node.tagName}.${node.className}`;
            }
          }
        }
        return { checked, worst };
      };
      const root = document.documentElement;
      const was = root.getAttribute("data-theme");
      const seen = {};
      for (const theme of ["dark", "light"]) {
        root.setAttribute("data-theme", theme);
        await settle();
        seen[theme] = measure();
      }
      if (was === null) root.removeAttribute("data-theme");
      else root.setAttribute("data-theme", was);
      return seen;
    });
    ok(
      "C9: every word on a step's line and a thought's line is at least 4.5:1 against what it stands on, in the dark theme and in the light one",
      ["dark", "light"].every((theme) => inks[theme].checked >= 20 && inks[theme].worst.ratio >= 4.5),
      JSON.stringify(inks),
    );

    const languages = await page.evaluate(async () => {
      const { rows, lineOf, wordsOf, settle } = window.__STEPS__;
      const said = () => rows().filter((row) => row.matches(".is-tool, .is-thinking")).map((row) => wordsOf(lineOf(row)));
      const seen = { ko: said() };
      for (const code of ["en", "ja", "zh", "es"]) {
        setLocale(code);
        await pollHelperPages();
        await settle();
        seen[code] = said();
      }
      setLocale("ko");
      return seen;
    });
    ok(
      "C9: every word a step and a thought bring is in all five languages — the same eight lines read in English, Japanese, Chinese and Spanish carry no Korean and are worded differently from the Korean ones",
      languages.ko.length === 8 && ["en", "ja", "zh", "es"].every((code) => languages[code].length === 8 &&
        !languages[code].some((line) => /[가-힣]/.test(line)) && languages[code].join("|") !== languages.ko.join("|")),
      JSON.stringify(languages),
    );
    ok("C1: the steps raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C5, C6 — the foot line while the turn is out: the step that is out, the
 * thought that is going, and the turn's end. */
async function stepsLive(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, stepsFixture(), { status: "working" });
    await installStepsProbe(page);
    const seen = await page.evaluate(async () => {
      const { list, rows, lineOf, wordsOf, tall, shown, settle } = window.__STEPS__;
      const seen = {};
      const status = () => list().querySelector(":scope > .helper-status");
      const feed = async (...turns) => {
        window.__CONVERSATION__.turns.push(...turns);
        await pollHelperPages();
        await settle();
      };
      const FOOTER = "/Users/dev/shop-app/src/screens/profile/Footer.tsx";
      const AT = 1_790_000_100_000;
      await feed({ role: "tool", text: `Read · ${FOOTER}`, at_ms: AT, tool: { call_id: "live1", name: "Read", kind: "read", input: FOOTER, is_error: false } });
      seen.shown = status()?.hidden === false && status().checkVisibility();
      seen.nowWords = wordsOf(status());
      seen.namesTheCall = seen.nowWords.includes("Footer.tsx");
      seen.moving = status()?.getAnimations({ subtree: true }).filter((one) => one.playState === "running").length ?? -1;
      // The row of the call that is out is a row like any other: open it, and
      // it stays open — the same row — when the call's answer joins it.
      const liveRow = rows().findLast((row) => row.classList.contains("is-tool") && row.classList.contains("is-live"));
      seen.liveRow = liveRow !== undefined;
      lineOf(liveRow)?.click();
      await settle();
      seen.liveOpened = liveRow?.open === true;
      await feed({
        role: "tool_result", at_ms: AT + 900, tool: { call_id: "live1", is_error: false },
        text: "     1→export const Footer = () => null;\n     2→// MARK_TWO Footer.tsx\n     3→\n     4→export default Footer;",
      });
      seen.answerKeptOpen = liveRow?.isConnected === true && liveRow.open === true && !liveRow.classList.contains("is-live") &&
        shown(liveRow, "MARK_TWO Footer.tsx");
      seen.nowAfter = wordsOf(status());
      seen.staleNow = seen.nowAfter.includes("Footer.tsx");
      // What the model is thinking stands as one closed line too.
      window.__CONVERSATION__.live = [{ role: "thinking", text: "First I check the grid. Then I read the cell." }];
      await pollHelperPages();
      await settle();
      const thinking = list().querySelector(":scope > .is-streaming.is-thinking");
      seen.thinking = {
        tag: thinking?.tagName, open: thinking?.open === true, height: thinking ? tall(thinking) : -1,
        line: wordsOf(lineOf(thinking)),
      };
      seen.nowThinking = wordsOf(status());
      // The turn ends: the foot line and the live rows go with it.
      window.__CONVERSATION__.status = "idle";
      window.__CONVERSATION__.live = [];
      await pollHelperPages();
      await settle();
      seen.endedHidden = status()?.hidden === true;
      seen.endedRows = list().querySelectorAll(":scope > .is-streaming").length;
      return seen;
    });
    ok(
      "C5: while the turn is out the foot line names the step that is out — the file being read — and stops naming it when that step's answer comes",
      seen.shown && seen.namesTheCall && !seen.staleNow,
      JSON.stringify(seen),
    );
    ok(
      "C5: the row of a step that is still out opens like any other, and stays open — the same row — when its answer joins it",
      seen.liveRow && seen.liveOpened && seen.answerKeptOpen,
      JSON.stringify(seen),
    );
    ok(
      "C5: what the model is thinking stands as one closed line too — its newest sentence, not its first — and is what the foot line says while nothing else is out",
      seen.thinking.tag === "DETAILS" && !seen.thinking.open && seen.thinking.height >= 0 && seen.thinking.height <= 44 &&
        seen.thinking.line.includes("Then I read the cell") && !seen.thinking.line.includes("First I check the grid") &&
        seen.nowThinking.includes("Then I read the cell"),
      JSON.stringify(seen),
    );
    ok("C5: the foot line and the live thought leave when the turn ends", seen.endedHidden && seen.endedRows === 0, JSON.stringify(seen));
    ok("C6: with motion allowed the foot line turns — something on it is animating", seen.moving >= 1, JSON.stringify(seen));
    ok("C5: the live rows raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C6 — asked for less motion, the foot line holds still. */
async function stepsStill(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await openConversation(page, stepsFixture(), { status: "working" });
    await installStepsProbe(page);
    const seen = await page.evaluate(async () => {
      const { list, wordsOf, settle } = window.__STEPS__;
      const seen = { asked: matchMedia("(prefers-reduced-motion: reduce)").matches };
      const status = () => list().querySelector(":scope > .helper-status");
      const running = () => status()?.getAnimations({ subtree: true }).filter((one) => one.playState === "running").length ?? -1;
      const FOOTER = "/Users/dev/shop-app/src/screens/profile/Footer.tsx";
      window.__CONVERSATION__.turns.push({
        role: "tool", text: `Read · ${FOOTER}`, at_ms: 1_790_000_100_000, tool: { call_id: "live1", name: "Read", kind: "read", input: FOOTER, is_error: false },
      });
      await pollHelperPages();
      await settle();
      seen.names = wordsOf(status()).includes("Footer.tsx");
      const first = wordsOf(status());
      seen.running = running();
      await new Promise((done) => setTimeout(done, 700));
      seen.same = wordsOf(status()) === first;
      seen.runningLater = running();
      return seen;
    });
    ok(
      "C6: asked for less motion, the foot line holds still — nothing on it animates and what it says does not change over the next moments — and it still names the step that is out",
      seen.asked && seen.names && seen.running === 0 && seen.same && seen.runningLater === 0,
      JSON.stringify(seen),
    );
    ok("C6: the still foot line raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C7 — a finished helper's page opens on its report. */
async function stepsReport(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openHelperConversation(page, stepsFixture(), "done");
    await installStepsProbe(page);
    const seen = await page.evaluate(async ({ words, first }) => {
      const { list, rows, steps, thoughts, wordsOf, holding, shown, settle } = window.__STEPS__;
      const seen = {};
      await settle();
      const visible = (nodes) => nodes.filter((node) => node.checkVisibility({ checkVisibilityCSS: true })).length;
      const report = list().querySelector(":scope > .helper-report");
      seen.report = report !== null;
      seen.wholeReport = words.every((sentence) => shown(report, sentence));
      // The first row that is not the person's brief is the report; the steps
      // and thoughts are folded under it, out of sight.
      const head = [...list().children].find((child) => !child.classList.contains("is-user"));
      seen.reportFirst = report !== null && head === report;
      seen.foldedSteps = visible(steps());
      seen.foldedThoughts = visible(thoughts());
      seen.foldedWords = shown(list(), first[1]);
      const door = report?.querySelector("[aria-expanded]");
      seen.door = { tag: door?.tagName ?? null, expanded: door?.getAttribute("aria-expanded") ?? null, words: wordsOf(door) };
      // Pressing the door unfolds them under the report, in their order, and
      // the report's words are still said once.
      door?.click();
      await settle();
      seen.unfolded = {
        expanded: door?.getAttribute("aria-expanded") ?? null,
        steps: visible(steps()),
        thoughts: visible(thoughts()),
        below: report !== null && steps().every((row) => row.getBoundingClientRect().top >= report.getBoundingClientRect().bottom - 1),
        said: holding(list(), words[1]).filter((node) => node.checkVisibility({ checkVisibilityCSS: true })).length,
      };
      door?.click();
      await settle();
      seen.refolded = { expanded: door?.getAttribute("aria-expanded") ?? null, steps: visible(steps()) };
      seen.rows = rows().length;
      return seen;
    }, { words: REPORT_WORDS, first: READ_FIRST });
    ok(
      "C7: a finished helper's page has its report on top — the answer it ended on, whole, the first row after the person's brief — with every step and thought folded under it and out of sight",
      seen.report && seen.wholeReport && seen.reportFirst && seen.foldedSteps === 0 && seen.foldedThoughts === 0 && !seen.foldedWords,
      JSON.stringify(seen),
    );
    ok(
      "C7: the report's door says how many steps it folds (twelve calls), announces closed and open, unfolds them below the report — the words of the report still said once — and folds them back",
      seen.door.tag === "BUTTON" && seen.door.expanded === "false" && /\b12\b/.test(seen.door.words) &&
        seen.unfolded.expanded === "true" && seen.unfolded.steps === 6 && seen.unfolded.thoughts === 2 && seen.unfolded.below &&
        seen.unfolded.said === 1 && seen.refolded.expanded === "false" && seen.refolded.steps === 0,
      JSON.stringify(seen),
    );
    ok("C7: the report raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C8 — by keyboard: every step's line is reachable in order, Enter and Space
 * open and close it, the engine says which it is, and the focus stays. */
async function stepsByKeyboard(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("DOM.enable");
    await cdp.send("Accessibility.enable");
    await openConversation(page, stepsFixture());
    await installStepsProbe(page);
    await page.evaluate(() => {
      const { steps, lineOf } = window.__STEPS__;
      steps().forEach((row, at) => lineOf(row)?.setAttribute("data-probe", `step-${at}`));
      lineOf(steps()[0])?.focus();
    });
    const focused = () => page.evaluate(() => document.activeElement?.getAttribute("data-probe") ?? document.activeElement?.tagName ?? null);
    const settle = () => page.evaluate(() => window.__STEPS__.settle());
    const path = [await focused()];
    await page.keyboard.press("Tab");
    path.push(await focused());
    await page.keyboard.press("Tab");
    path.push(await focused());
    await page.keyboard.press("Shift+Tab");
    path.push(await focused());
    // Enter opens the bash step's row, Space closes it; the focus stays on
    // its line throughout, and a poll that brings new words does not take it.
    await page.evaluate(() => window.__STEPS__.lineOf(window.__STEPS__.steps()[3])?.focus());
    const state = async () => ({
      open: await page.evaluate(() => window.__STEPS__.steps()[3]?.open === true),
      focus: await focused(),
      ax: await axOf(cdp, '[data-probe="step-3"]'),
    });
    const states = [await state()];
    await page.keyboard.press("Enter");
    await settle();
    states.push(await state());
    await page.evaluate(async () => {
      window.__CONVERSATION__.turns.push({ role: "assistant", text: "다음으로 넘어갑니다.", at_ms: 1_790_000_060_000 });
      await pollHelperPages();
    });
    await settle();
    states.push(await state());
    await page.keyboard.press(" ");
    await settle();
    states.push(await state());
    ok(
      "C8: every step's line is reachable by the Tab key in order — one stop per row, none inside a closed one — and Shift+Tab comes back",
      JSON.stringify(path) === JSON.stringify(["step-0", "step-1", "step-2", "step-1"]),
      JSON.stringify(path),
    );
    ok(
      "C8: Enter opens the focused step's row and Space closes it, the engine's accessibility tree says closed, open, open (across a poll) and closed, the name it reads out carries the step's command, and the focus stays on the line the whole time",
      JSON.stringify(states.map((one) => one.open)) === JSON.stringify([false, true, true, false]) &&
        JSON.stringify(states.map((one) => one.ax?.expanded)) === JSON.stringify([false, true, true, false]) &&
        states.every((one) => one.focus === "step-3" && one.ax?.name.includes("SCREEN_WIDTH")),
      JSON.stringify(states),
    );
    ok("C8: the keyboard raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C10 — one painter: the same turns on the wire's page, a helper's page and a
 * pane's page come to the same rows. */
async function stepsAcrossPages(browser, origin, ok) {
  const seen = {};
  const faulted = [];
  for (const kind of ["wire", "helper", "pane"]) {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      if (kind === "wire") await openConversation(page, stepsFixture(), { status: "working" });
      else if (kind === "helper") await openHelperConversation(page, stepsFixture(), "working");
      else await openPaneConversation(page, stepsFixture());
      await installStepsProbe(page);
      seen[kind] = await page.evaluate(async () => {
        const { list, rows, lineOf, wordsOf, settle } = window.__STEPS__;
        await settle();
        return {
          rows: rows().map((row) => [
            row.tagName,
            [...row.classList].filter((name) => name !== "is-live").sort().join("."),
            wordsOf(row.matches(".is-user, .is-assistant") ? row : lineOf(row)),
          ].join("|")),
          report: list().querySelector(":scope > .helper-report") !== null,
        };
      });
      faulted.push(...faults);
    } finally {
      await page.close();
    }
  }
  ok(
    "C10: the wire's page, a helper's page and a pane's page are one painter — the same turns come to the same ten rows on all three (the person's brief, two thoughts, the reads as two rows of their own and the one that failed, the search, the page, the edit and the answer), and a helper still working has no report",
    seen.wire.rows.length === 10 && JSON.stringify(seen.helper.rows) === JSON.stringify(seen.wire.rows) &&
      JSON.stringify(seen.pane.rows) === JSON.stringify(seen.wire.rows) && !seen.helper.report,
    JSON.stringify(seen),
  );
  ok("C10: the three pages raised no page errors", faulted.length === 0, faulted.join("\n"));
}

/* C11 — one table of tool names. The core reduces every vendor's name for a
 * tool to one word (`hook::Tool::named`) and a turn carries it as `tool.kind`;
 * the page draws by that word and keeps no list of names of its own, so a name
 * only the core knows wears the look its kind wears. Each call stands alone (a
 * sentence between them, so none folds). */
const KIND_CALLS = [
  // The name the CLI wrote, the kind the core reduced it to, its target, and
  // the look the row must wear (null: a kind the page has no word for, drawn
  // as the tool's own spelling).
  { name: "OpenDocument", kind: "read", target: `${SHOP}/README.md`, look: "read" },
  { name: "ripgrep", kind: "grep", target: "SCREEN_WIDTH in src", look: "grep" },
  { name: "LocalShell", kind: "bash", target: "yarn test", look: "bash" },
  { name: "GoogleWebSearch", kind: "websearch", target: "react native flatlist keys", look: "websearch" },
  { name: "UrlFetch", kind: "web", target: "https://example.com/docs", look: "web" },
  { name: "SubAgent", kind: "task", target: "check the list keys", look: "task" },
  // The page does not read the name: a call whose name says one thing and whose
  // kind another wears the kind's look.
  { name: "Read", kind: "grep", target: "the name is only the CLI's spelling", look: "grep" },
  // A tool the core has no word for keeps its own spelling as its kind.
  { name: "Lookup", kind: "Lookup", target: "ui/shell.js", look: null },
  // A call written before turns carried a kind has none to read: it says its own
  // name, and the page does not guess a look from it.
  { name: "Read", kind: undefined, target: `${SHOP}/a.rs`, look: null },
];

async function stepsByKind(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const turns = [{ role: "user", text: "이 도구들이 무엇을 하는지 보여 줘.", at_ms: CLOCK }];
    KIND_CALLS.forEach((call, at) => {
      const id = `k${at}`;
      turns.push({
        role: "tool", text: `${call.name} · ${call.target}`, at_ms: CLOCK + 2000 * (at + 1),
        tool: { call_id: id, name: call.name, ...(call.kind !== undefined && { kind: call.kind }), input: call.target, is_error: false },
      });
      turns.push({ role: "tool_result", text: "ok", at_ms: CLOCK + 2000 * (at + 1) + 300, tool: { call_id: id, is_error: false } });
      turns.push({ role: "assistant", text: `다음 ${at + 1}`, at_ms: CLOCK + 2000 * (at + 1) + 600 });
    });
    await openConversation(page, turns);
    await installStepsProbe(page);
    const seen = await page.evaluate(async () => {
      const { steps, lineOf, settle } = window.__STEPS__;
      await settle();
      return {
        rows: steps().map((row) => ({
          mark: lineOf(row).querySelector(".helper-step-icon use")?.getAttribute("href") ?? null,
          word: lineOf(row).querySelector(".helper-step-kind")?.textContent ?? null,
          folded: row.classList.contains("is-run"),
        })),
        looks: {
          read: ["#i-file", t("worker.stepRead", "파일 읽기")],
          grep: ["#i-search", t("worker.stepSearch", "검색")],
          bash: ["#i-terminal", t("worker.stepShell", "셸 실행")],
          web: ["#i-globe", t("worker.stepWeb", "웹 읽기")],
          websearch: ["#i-globe", t("worker.stepWebSearch", "웹 검색")],
          task: ["#i-bot", t("worker.stepTask", "헬퍼 호출")],
        },
        table: typeof STEP_NAMES,
        sidebar: [activityWord("websearch"), t("activity.websearch", "웹 검색"), agentActivityKind("websearch"), agentActivityKind("web")],
      };
    });
    const wanted = KIND_CALLS.map((call) => {
      const [mark, word] = call.look ? seen.looks[call.look] : ["#i-wrench", call.kind ?? call.name];
      return { mark, word, folded: false };
    });
    const wrong = KIND_CALLS.filter((call, at) => JSON.stringify(seen.rows[at]) !== JSON.stringify(wanted[at]))
      .map((call) => `${call.name}/${call.kind}`);
    ok(
      "C11: the page draws a call by the kind the core reduced its tool to, not by a name it knows — a name only the core knows (OpenDocument, ripgrep, LocalShell, GoogleWebSearch, UrlFetch, SubAgent) wears its kind's mark and words, a call whose kind contradicts its name wears the kind's look, a tool with no word keeps its own spelling with the wrench, and a call that carries no kind says its own name",
      seen.rows.length === KIND_CALLS.length && wrong.length === 0,
      JSON.stringify({ wrong, rows: seen.rows, wanted }),
    );
    ok(
      "C11: the page holds no table of vendor tool names — the one it had is gone",
      seen.table === "undefined",
      `typeof STEP_NAMES is ${seen.table}`,
    );
    ok(
      "C11: the sidebar draws the core's word for a search of the web in its own words, not as the raw \"websearch\", and files it where a fetch of a page is filed",
      seen.sidebar[0] === seen.sidebar[1] && seen.sidebar[0] !== "websearch" && seen.sidebar[2] === seen.sidebar[3] && seen.sidebar[2] !== "other",
      JSON.stringify(seen.sidebar),
    );
    ok("C11: the kinds raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C12 — a thought's line is the terminal's, held to its examples. The
 * terminal's `live_heading` (zo-ide `tui/thinking.rs`, t-5872) and the page's
 * `thoughtHeading` each say what a reasoning block is about in one line. The
 * cases are the terminal's own — each row names the Rust test it is read from,
 * its text and what `live_heading` answers (null: it keeps the row's current
 * word) — so the two are held to the same examples, and a change to one that
 * leaves the other behind fails here. Two differences are on purpose:
 *   1. Width. The terminal budgets 48 display columns and cuts at a word; the
 *      page's line is cut only at THOUGHT_LINE_MAX characters (the row's own
 *      ellipsis is what a reader sees).
 *   2. A thought that is over has no "current word" to keep: where the
 *      terminal answers nothing, the row says the thought's unfinished tail
 *      (`thoughtHeading(text, true)`). A thought still going says nothing, like
 *      the terminal, until a sentence closes. */
const LIVE_HEADING_TESTS = {
  bold: "a_bold_heading_outranks_the_sentences_only_where_codex_puts_it",
  newest: "headerless_thinking_says_its_newest_complete_sentence",
  korean: "korean_sentences_close_and_wide_text_is_cut_by_columns",
  open: "a_long_open_sentence_is_cut_rather_than_withheld",
};
const LIVE_HEADING_CASES = [
  [LIVE_HEADING_TESTS.bold, "Hmm.\n**Polishing tool display**\nThen more.", "Polishing tool display"],
  [LIVE_HEADING_TESTS.bold, "The **key** point is the cache. Next", "The key point is the cache"],
  [LIVE_HEADING_TESTS.newest, "The user wants me to", null],
  [LIVE_HEADING_TESTS.newest, "The user wants me to read the test. Let me", "The user wants me to read the test"],
  [LIVE_HEADING_TESTS.newest, "The user wants me to read the test. Let me open src/app.rs first.\nNow", "Let me open src/app.rs first"],
  [LIVE_HEADING_TESTS.newest, "- **Plan**: read the failing test\n", "Plan: read the failing test"],
  [LIVE_HEADING_TESTS.newest, "1. Inspect the wiring\n2. Fix", "Inspect the wiring"],
  [LIVE_HEADING_TESTS.newest, "Reading src/app.rs and v1.2 now", null],
  [LIVE_HEADING_TESTS.korean, "먼저 실패하는 시험을 읽어야 한다. 그다음", "먼저 실패하는 시험을 읽어야 한다"],
  [LIVE_HEADING_TESTS.korean, "파일을 읽고 있다。다음은", "파일을 읽고 있다"],
  [LIVE_HEADING_TESTS.open, "short and still open", null],
];
// `leading_bold_heading`'s own lines in the first of those tests: a bold run
// counts only where it opens a line, and only once it is closed.
const LEADING_BOLD_CASES = [
  ["**Title**\nbody", "Title"],
  ["  **Title** body", "Title"],
  ["the **key** point", null],
  ["**still open", null],
];
// The two Rust cases that are cut by columns (`korean_…`, `a_long_open_…`) — the
// page keeps both whole, under its own cap.
const KOREAN_LONG = "이 문장은 아주 길어서 상태 줄의 인터럽트 힌트를 밀어낼 만큼 넓은 폭을 차지하므로 잘려야 한다.";
const OPEN_LONG = "this opening sentence goes on well past the cap without a stop";

async function stepsThoughtLine(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const turns = [{ role: "user", text: "무슨 생각을 하고 있어?", at_ms: CLOCK }];
    LIVE_HEADING_CASES.forEach(([, text], at) => turns.push({ role: "thinking", text, at_ms: CLOCK + 1000 * (at + 1) }));
    await openConversation(page, turns, { status: "working" });
    await installStepsProbe(page);
    const seen = await page.evaluate(async ({ cases, bold, korean, open }) => {
      const { thoughts, list, settle } = window.__STEPS__;
      await settle();
      const seen = {};
      seen.going = cases.map(([, text]) => thoughtHeading(text));
      seen.over = cases.map(([, text]) => thoughtHeading(text, true));
      seen.bold = bold.map(([text]) => thoughtLeadingBold(text));
      seen.drawn = thoughts().map((row) => row.querySelector(".helper-step-target")?.textContent ?? null);
      seen.cap = THOUGHT_LINE_MAX;
      seen.wide = [thoughtHeading(korean), thoughtHeading(open), thoughtHeading(open, true)];
      const run = "word ".repeat(120).trim();
      seen.capped = [thoughtHeading(run), thoughtHeading(`${run}. Next`), thoughtHeading(run, true)];
      const going = async (text) => {
        window.__CONVERSATION__.live = [{ role: "thinking", text }];
        await pollHelperPages();
        await settle();
        return list().querySelector(":scope > .is-streaming.is-thinking .helper-step-target")?.textContent ?? null;
      };
      seen.live = [
        await going(cases[2][1]),
        await going(cases[3][1]),
        await going(cases[4][1]),
      ];
      return seen;
    }, { cases: LIVE_HEADING_CASES, bold: LEADING_BOLD_CASES, korean: KOREAN_LONG, open: OPEN_LONG });
    const differ = (got, wanted) => LIVE_HEADING_CASES.filter((one, at) => got[at] !== wanted[at]).map((one) => `${one[0]}: ${JSON.stringify(one[1])}`);
    const said = LIVE_HEADING_CASES.map((one) => one[2]);
    const over = LIVE_HEADING_CASES.map((one) => one[2] ?? one[1]);
    ok(
      "C12: a thought that is going says what the terminal's live_heading says for the same text — the eleven cases of its four tests (a bold heading that opens a line; else the newest complete sentence, a list marker and emphasis stars left off, a dot in a path or a version not a full stop, a full-width stop closing one) — and nothing at all until a sentence has closed",
      differ(seen.going, said).length === 0,
      JSON.stringify({ differ: differ(seen.going, said), going: seen.going }),
    );
    ok(
      "C12: a bold run counts only where it opens a line and only once it is closed — leading_bold_heading's own four cases",
      JSON.stringify(seen.bold) === JSON.stringify(LEADING_BOLD_CASES.map((one) => one[1])),
      JSON.stringify(seen.bold),
    );
    ok(
      "C12: a thought that is over has a line where the terminal says nothing — its unfinished tail — and the row drawn for each of the eleven says it",
      differ(seen.over, over).length === 0 && JSON.stringify(seen.drawn) === JSON.stringify(over),
      JSON.stringify({ differ: differ(seen.over, over), over: seen.over, drawn: seen.drawn }),
    );
    ok(
      "C12: the row of a thought still going does not flash a half-written sentence — it says nothing until the first sentence closes, then that sentence, then the newest",
      JSON.stringify(seen.live) === JSON.stringify(["", "The user wants me to read the test", "Let me open src/app.rs first"]),
      JSON.stringify(seen.live),
    );
    ok(
      "C12: width is the one difference on purpose — the terminal cuts at 48 columns and a word, the page keeps a sentence whole up to its own cap and cuts a longer run with an ellipsis at that cap, closed or open",
      seen.wide[0] === KOREAN_LONG.slice(0, -1) && seen.wide[1] === null && seen.wide[2] === OPEN_LONG &&
        seen.capped.every((line) => line?.length === seen.cap && line.endsWith("…")),
      JSON.stringify({ wide: seen.wide, capped: seen.capped.map((line) => line?.length), cap: seen.cap }),
    );
    ok("C12: the thought lines raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C13 — a thought being read does not close under the reader. The words that
 * streamed into an open line close into their turn; the row that stands for
 * the turn, in the streaming row's place, is open the way that one was and
 * holds the keyboard on its line where that one held it. A thought nobody
 * opened closes into a closed row and takes nothing. */
async function stepsThoughtHandOver(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, stepsFixture(), { status: "working" });
    await installStepsProbe(page);
    const seen = await page.evaluate(async ({ at }) => {
      const { list, lineOf, shown, settle } = window.__STEPS__;
      const streaming = () => list().querySelector(":scope > .is-streaming.is-thinking");
      const settled = () => [...list().querySelectorAll(":scope > .is-thinking:not(.is-streaming)")];
      const wait = (ms) => new Promise((done) => setTimeout(done, ms));
      const stream = async (text) => {
        window.__CONVERSATION__.live = [{ role: "thinking", text }];
        await pollHelperPages();
        await settle();
      };
      // The poll that brings the turn brings the empty live list with it.
      const close = async (text, when) => {
        window.__CONVERSATION__.turns.push({ role: "thinking", text, at_ms: when });
        window.__CONVERSATION__.live = [];
        await pollHelperPages();
        await settle();
      };
      const FIRST = "Every cell remounts on each scroll tick because its key is an index.";
      const NEWEST = "Keying the cells by id should keep them.";
      const SECOND = "One more look at the cell before the edit.";
      const seen = {};
      const thoughts = settled().length;
      await stream(`${FIRST} ${NEWEST}`);
      const going = streaming();
      lineOf(going).click();
      await wait(60);
      await settle();
      lineOf(going).focus();
      seen.read = { open: going.open === true, held: document.activeElement === lineOf(going), words: shown(going, FIRST) };
      await close(`${FIRST} ${NEWEST}`, at);
      const row = settled().at(-1);
      seen.turn = {
        added: settled().length === thoughts + 1,
        gone: streaming() === null,
        open: row?.open === true,
        words: row ? shown(row, FIRST) : false,
        held: row ? document.activeElement === lineOf(row) : false,
      };
      await stream(SECOND);
      const idle = streaming();
      seen.idle = { open: idle?.open === true, held: idle ? document.activeElement === lineOf(idle) : false };
      await close(SECOND, at + 3000);
      const closed = settled().at(-1);
      seen.after = {
        added: settled().length === thoughts + 2,
        open: closed?.open === true,
        held: closed ? document.activeElement === lineOf(closed) : false,
      };
      return seen;
    }, { at: CLOCK + 200_000 });
    ok(
      "C13: a thought being read does not close under the reader — when its words close into their turn the row that stands for the turn is open, with the whole thought in it, and holds the keyboard on its line where the streaming row held it",
      seen.read.open && seen.read.held && seen.read.words && seen.turn.added && seen.turn.gone && seen.turn.open &&
        seen.turn.words && seen.turn.held,
      JSON.stringify(seen),
    );
    ok(
      "C13: a thought nobody opened closes into a closed row and takes no keyboard — the hand-over is for a reader who asked",
      !seen.idle.open && !seen.idle.held && seen.after.added && !seen.after.open && !seen.after.held,
      JSON.stringify(seen),
    );
    ok("C13: the thought's hand-over raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C14 — a streaming thought's open body is written once a frame, as a
 * streaming answer is, and never under a selection: deltas that arrive faster
 * than the frame come to one write of the newest words, and words a person
 * chose in the body stay chosen — the frame that would collapse them waits. */
async function stepsThoughtWrites(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, stepsFixture(), { status: "working" });
    await installStepsProbe(page);
    const seen = await page.evaluate(async () => {
      const { list, lineOf, settle } = window.__STEPS__;
      const wait = (ms) => new Promise((done) => setTimeout(done, ms));
      const run = list().__run;
      // One delta of the thought that is going, as the wire's poll brings it.
      const bring = (text) => {
        window.__CONVERSATION__.live = [{ role: "thinking", text }];
        run.wireLog = { ...run.wireLog, live: window.__CONVERSATION__.live };
        syncStreamingTurns(list(), run);
      };
      const seen = {};
      let text = "First words of the thought.";
      bring(text);
      const going = list().querySelector(":scope > .is-streaming.is-thinking");
      lineOf(going).click();
      await wait(60);
      await settle();
      const body = going.querySelector(":scope > .helper-thought-body");
      seen.opened = going.open === true && body.textContent === text;
      const watch = new MutationObserver(() => {});
      watch.observe(body, { childList: true, characterData: true, subtree: true });
      for (let delta = 0; delta < 20; delta += 1) {
        text += ` Sentence ${delta} of the thought, one more delta.`;
        bring(text);
      }
      seen.inTheFrame = watch.takeRecords().length;
      await settle();
      seen.byTheFrame = watch.takeRecords().length;
      seen.newest = body.textContent === text;
      // Words a person chose in the body.
      const node = body.firstChild;
      const selection = getSelection();
      selection.setBaseAndExtent(node, 6, node, 20);
      const chosen = selection.toString();
      watch.takeRecords();
      text += " A newer sentence arrives while these words are chosen.";
      bring(text);
      await settle();
      seen.chosen = chosen;
      seen.kept = chosen !== "" && selection.toString() === chosen && body.contains(selection.anchorNode);
      seen.rewrittenUnderIt = watch.takeRecords().length;
      // Let go of them: the next delta brings the newest words in.
      selection.removeAllRanges();
      text += " And a last one, once they are let go.";
      bring(text);
      await settle();
      seen.caughtUp = body.textContent === text;
      return seen;
    });
    ok(
      "C14: twenty deltas that arrive inside one frame are one write of the open body of a thought that is going — not twenty — and the body holds the newest words",
      seen.opened && seen.inTheFrame + seen.byTheFrame <= 2 && seen.newest,
      JSON.stringify(seen),
    );
    ok(
      "C14: words a person chose in that body stay chosen when a newer delta arrives — the frame that would collapse the selection is skipped, and the next delta after they are let go brings the newest words in",
      seen.kept && seen.rewrittenUnderIt === 0 && seen.caughtUp,
      JSON.stringify(seen),
    );
    ok("C14: the streaming thought raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C15 — turning the Focus view on keeps what a person had open. The Focus
 * view keeps its rows single, so a row of steps gives its members back as
 * rows of their own: the member that was open comes back open, the others
 * closed, and the keyboard, where it was on the row or on that member, is held
 * by what stands for it — the row's line, or the head of the fold that now
 * covers it. */
async function stepsFocusKeepsOpen(browser, origin, ok) {
  const seen = {};
  const faulted = [];
  for (const scenario of ["member", "run"]) {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await openConversation(page, stepsFixture(), { status: "idle" });
      await installStepsProbe(page);
      seen[scenario] = await page.evaluate(async ({ scenario, file }) => {
        const { list, rows, lineOf, shown, settle } = window.__STEPS__;
        const wait = (ms) => new Promise((done) => setTimeout(done, ms));
        const out = {};
        const run = rows().find((row) => row.classList.contains("is-run"));
        out.hasRun = run !== undefined;
        lineOf(run).click();
        await wait(60);
        await settle();
        const members = [...run.querySelectorAll(":scope > .helper-run-body > .helper-turn")];
        out.members = members.length;
        const seqs = members.map((member) => member.dataset.turn);
        if (scenario === "member") {
          lineOf(members[2]).click();
          await wait(60);
          await settle();
          lineOf(members[2]).focus();
        } else {
          lineOf(run).focus();
        }
        out.before = {
          run: run.open,
          open: members.map((member) => member.open),
          held: document.activeElement === lineOf(scenario === "member" ? members[2] : run),
        };
        document.querySelector("#worker-view .worker-focus").click();
        await wait(60);
        await settle();
        const row = (seq) => list().querySelector(`:scope > [data-turn="${seq}"]`);
        // What stands for a row on screen: its line, or — when the Focus view
        // has folded it under a head — the head.
        const standing = (seq) => {
          const one = row(seq);
          return one === null ? null : one.hidden ? one.__group?.firstElementChild ?? null : lineOf(one);
        };
        const stands = standing(scenario === "member" ? seqs[2] : seqs[0]);
        out.after = {
          runs: list().querySelectorAll(":scope > .is-run").length,
          singles: seqs.map((seq) => row(seq) !== null),
          open: seqs.map((seq) => row(seq)?.open === true),
          held: stands !== null && document.activeElement === stands,
          active: document.activeElement?.tagName ?? null,
        };
        if (scenario === "member") {
          // The fold opened, the member is open in it with its file.
          row(seqs[2])?.__group?.firstElementChild?.click();
          await wait(60);
          await settle();
          out.shown = row(seqs[2]) !== null && shown(row(seqs[2]), `MARK_TWO ${file}`);
        }
        return out;
      }, { scenario, file: READ_FIRST[2] });
      faulted.push(...faults);
    } finally {
      await page.close();
    }
  }
  const only = (open, at) => JSON.stringify(open) === JSON.stringify([0, 1, 2, 3, 4, 5].map((one) => one === at));
  ok(
    "C15: the Focus view gives a row of steps back as single rows, and the member a person had open comes back open — the others closed — with its file in it when the fold that covers it is pressed",
    seen.member.hasRun && seen.member.members === 6 && seen.member.before.open[2] === true && seen.member.after.runs === 0 &&
      seen.member.after.singles.every(Boolean) && only(seen.member.after.open, 2) && seen.member.shown === true,
    JSON.stringify(seen.member),
  );
  ok(
    "C15: the keyboard, on the line of the member that was open, is held by what stands for it when the Focus view comes on — not dropped to the body",
    seen.member.before.held && seen.member.after.held,
    JSON.stringify(seen.member),
  );
  ok(
    "C15: the keyboard on a row of steps itself is held by what stands for that row when the Focus view comes on — the first of its members, or the fold's head — and no member is opened that was not",
    seen.run.hasRun && seen.run.before.held && seen.run.after.singles.every(Boolean) && only(seen.run.after.open, -1) &&
      seen.run.after.held,
    JSON.stringify(seen.run),
  );
  ok("C15: the Focus view raised no page errors", faulted.length === 0, faulted.join("\n"));
}

/* Enough turns before the fixture's own that the list scrolls. */
function paddedFixture() {
  const filler = Array.from({ length: 30 }, (_, at) => ({
    role: at % 2 === 0 ? "user" : "assistant",
    text: `이전 대화 ${at} — ${"길게 이어지는 문장이 목록을 스크롤할 만큼 쌓인다. ".repeat(3)}`,
  }));
  return [...filler, ...stepsFixture()];
}

/* C17 — a Space on a step's line is the line's own. It opens or closes the
 * row; it is not a wish to go towards the foot (the list's Space) nor to leave
 * it (Shift+Space), so a reader a little above the foot stays there and a list
 * that follows its foot goes on following it. */
async function stepsSpaceOnALine(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, paddedFixture(), { status: "idle" });
    await installStepsProbe(page);
    // The reader gets a little above the foot the way a person does, by a wheel
    // upward, which leaves at once. A place set from script is no such wish: the
    // list reads an upward move under a list that changed size as the list's own.
    const middle = await page.evaluate(async () => {
      const { list, settle } = window.__STEPS__;
      const box = list();
      box.scrollTop = box.scrollHeight;
      await settle();
      const rect = box.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    });
    await page.mouse.move(middle.x, middle.y);
    await page.mouse.wheel(0, -30);
    const before = await page.evaluate(async () => {
      const { list, steps, lineOf } = window.__STEPS__;
      const box = list();
      // The list has come to rest off its foot, the keyboard on the last step's line.
      for (let last = -1, beat = 0; beat < 90; beat += 1) {
        if (box.scrollHeight - box.scrollTop - box.clientHeight > 1 && box.scrollTop === last) break;
        last = box.scrollTop;
        await window.__PAINTED__();
      }
      lineOf(steps().at(-1)).focus({ preventScroll: true });
      return {
        tall: box.scrollHeight > box.clientHeight + 200,
        away: chatAway(box),
        gap: Math.round(box.scrollHeight - box.scrollTop - box.clientHeight),
        held: document.activeElement === lineOf(steps().at(-1)),
      };
    });
    await page.keyboard.press(" ");
    await page.evaluate(() => window.__STEPS__.settle());
    const after = await page.evaluate(() => {
      const { list, steps } = window.__STEPS__;
      return { away: chatAway(list()), open: steps().at(-1).open === true };
    });
    // The list at its foot and following it, the keyboard on another step's line.
    const following = await page.evaluate(async () => {
      const { list, steps, lineOf, settle } = window.__STEPS__;
      const box = list();
      box.scrollTop = box.scrollHeight;
      await settle();
      lineOf(steps()[2]).focus({ preventScroll: true });
      return { away: chatAway(box), held: document.activeElement === lineOf(steps()[2]) };
    });
    await page.keyboard.down("Shift");
    await page.keyboard.press(" ");
    await page.keyboard.up("Shift");
    await page.evaluate(() => window.__STEPS__.settle());
    const shifted = await page.evaluate(() => ({ away: chatAway(window.__STEPS__.list()) }));
    ok(
      "C17: a Space on a step's line opens its row and does not count as a wish to go towards the foot — the reader a little above it is still away from it",
      before.tall && before.away && before.gap > 1 && before.gap < 50 && before.held && after.open && after.away,
      JSON.stringify({ before, after }),
    );
    ok(
      "C17: Shift+Space on a step's line does not count as leaving the foot — a list that follows its foot goes on following it",
      following.held && !following.away && !shifted.away,
      JSON.stringify({ following, shifted }),
    );
    ok("C17: the keys on the step's line raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C18 — with the agent graph on screen in another leaf, a Space on a step's
 * line still opens its row. The graph takes the Space of its own view (its
 * hand tool) — holding it still holds the tool with nothing focused — but not
 * the Space of a line, a button or a keyboard owner on another surface. */
async function stepsSpaceBesideTheGraph(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, stepsFixture(), { status: "idle" });
    await installStepsProbe(page);
    const stood = await page.evaluate(async () => {
      const wait = (ms) => new Promise((done) => setTimeout(done, ms));
      window.__ANSWER__.board_snapshot = () => ({
        columns: [{ bucket: "working", cards: [{
          pane: "term:3", agent: "codex", state: "working", heading: "Held snapshot",
          task: "Held snapshot", project: "/fixture", worktree: "main", at: 1, changed_at: 1,
        }] }], attention_count: 0, total_count: 1,
      });
      agentBoardMode = "graph";
      const home = focusedPane;
      const group = nextGroupId;
      nextGroupId += 1;
      setStageTree(splitStageLeaf(stageTree(), home, group, "horizontal", "second"));
      focusedPane = group;
      openBoard();
      await paintBoardView(boardTab(), { force: true });
      await wait(300);
      const view = visibleAgentGraphView();
      const list = document.querySelector("#worker-view .helper-turns");
      const { steps, lineOf } = window.__STEPS__;
      lineOf(steps()[3]).focus({ preventScroll: true });
      return {
        graph: view !== null,
        page: list?.checkVisibility() === true,
        apart: view !== null && list !== null && !view.contains(list) && !list.contains(view),
        held: document.activeElement === lineOf(steps()[3]),
        open: steps()[3].open === true,
      };
    });
    await page.keyboard.press(" ");
    await page.evaluate(() => window.__STEPS__.settle());
    const after = await page.evaluate(() => ({ open: window.__STEPS__.steps()[3].open === true }));
    // The hand tool is the graph's own: with nothing focused, holding Space
    // holds it and letting go lets go.
    await page.evaluate(() => document.activeElement?.blur());
    await page.keyboard.down(" ");
    const held = await page.evaluate(() => agentGraphSpaceHeld);
    await page.keyboard.up(" ");
    const released = await page.evaluate(() => agentGraphSpaceHeld);
    ok(
      "C18: the agent graph and a conversation page can be on screen together — a board drawing the graph in one leaf, the page in another",
      stood.graph && stood.page && stood.apart && stood.held && !stood.open,
      JSON.stringify(stood),
    );
    ok(
      "C18: with the graph on screen beside it, a Space on a step's line opens the row — the graph does not take a Space that is the line's own",
      after.open,
      JSON.stringify({ stood, after }),
    );
    ok(
      "C18: the hand tool is still the graph's own — with nothing focused, holding Space holds it and letting go lets go",
      held === true && released === false,
      JSON.stringify({ held, released }),
    );
    ok("C18: the graph beside the page raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* C16 — asked for less motion, the whole page holds still, and the mark of a
 * step that is still out with it (C6 holds the foot line to it): swept over
 * the page, the pulse of the mark on a step's line, the foot line's ring, the
 * fades and the dots. The same page is swept again with motion allowed and
 * moves, so the stillness is the guard's and not an empty page's — the guard
 * the person approved in the draft (`.run .dot, .spin { animation: none }`). */
async function stepsStillPage(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await openConversation(page, stepsFixture(), { status: "working" });
    await installStepsProbe(page);
    await page.evaluate(async () => {
      const FOOTER = "/Users/dev/shop-app/src/screens/profile/Footer.tsx";
      window.__CONVERSATION__.turns.push({
        role: "tool", text: `Read · ${FOOTER}`, at_ms: 1_790_000_100_000, tool: { call_id: "live1", name: "Read", kind: "read", input: FOOTER, is_error: false },
      });
      await pollHelperPages();
    });
    const sweep = () => page.evaluate(async () => {
      const { list, settle } = window.__STEPS__;
      await settle();
      const runningIn = (node, options) => node?.getAnimations(options).filter((one) => one.playState === "running") ?? [];
      const step = list().querySelector(":scope > .is-tool.is-live");
      return {
        asked: matchMedia("(prefers-reduced-motion: reduce)").matches,
        live: step !== null,
        page: runningIn(document.querySelector("#worker-view"), { subtree: true }).map((one) => one.animationName ?? one.transitionProperty ?? "?"),
        mark: runningIn(step?.querySelector(":scope > .helper-step-line > .helper-step-icon")).map((one) => one.animationName ?? "?"),
        status: runningIn(list().querySelector(":scope > .helper-status"), { subtree: true }).length,
      };
    });
    const still = await sweep();
    await page.emulateMedia({ reducedMotion: "no-preference" });
    const moving = await sweep();
    ok(
      "C16: asked for less motion, nothing on the conversation page animates — swept over the whole page with a step still out: no pulse on the step's mark, no ring or fade on the foot line, no dot",
      still.asked && still.live && still.page.length === 0 && still.mark.length === 0 && still.status === 0,
      JSON.stringify(still),
    );
    ok(
      "C16: with motion allowed the same page moves — the mark of the step that is out pulses and the foot line animates — so the stillness above is the guard's, not an empty page's",
      !moving.asked && moving.live && moving.mark.includes("helper-live-pulse") && moving.status >= 1,
      JSON.stringify(moving),
    );
    ok("C16: sweeping the page for motion raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* D — what a step's line says when it cannot know, and what a shell step is titled (t-18702). The
 * conversation view against its approved mockup (「도우미 대화 화면 시안」, v2): five places the page
 * still differed from it — a shell step's title carried the whole `cd … && …` command, a time nobody
 * knows was written 「0.0초」 (and a header clock said 「완료 0초」 on every finished helper), the now
 * line fell back to the agent's English word, a thought's length was worded backwards, and a list
 * written right under a sentence ran into the sentence. Synthetic like everything here: a shop app,
 * never a real path or address. */

/* D1 — a shell step is titled by what it did. A command that opens by changing folder — `cd <dir> &&
 * …`, `cd <dir> ; …` — says where it ran, not what it did: the title is the command's own first words,
 * fitted to STEP_TARGET_FIT, and when a folder was named, its last name after 「 · 」 (the mockup's
 * row: 「셸 grep SCREEN_WIDTH… · src」). The opened row keeps the whole command, as it always did. */
const SHELL_TITLES = [
  { id: "s1", command: `cd ${SHOP} && grep -rn "SCREEN_WIDTH\\|SCREEN_HEIGHT\\|SCREEN_W\\b" src | head -60` },
  { id: "s2", command: 'cd "/Users/dev/my shop" ; ls -la', title: "ls -la · my shop" },
  { id: "s3", command: `cd ${SHOP}/ios && cd Pods && make test`, title: "make test · Pods" },
  { id: "s4", command: "npm test -- --runInBand --testPathPattern=profile/Gallery --coverage" },
  // A `cd` that is not the command's opening is part of what the command does.
  { id: "s5", command: 'echo "cd /x && y" | wc -l', title: 'echo "cd /x && y" | wc -l' },
];

async function stepsShellTitle(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const turns = [{ role: "user", text: "셸로 확인해 줘.", at_ms: CLOCK }];
    let clock = CLOCK;
    const call = (id, name, kind, input, answer) => {
      turns.push({ role: "tool", text: `${name} · ${input}`, at_ms: (clock += 1000), tool: { call_id: id, name, kind, input, is_error: false } });
      turns.push({ role: "tool_result", text: answer, at_ms: (clock += 400), tool: { call_id: id, is_error: false } });
    };
    // A step of another kind stands between two shell steps, or they would fold into one row.
    for (const one of SHELL_TITLES) {
      call(one.id, "Bash", "bash", one.command, "ok");
      call(`${one.id}-read`, "Read", "read", `${PROFILE}/Gallery.tsx`, fileBody("Gallery.tsx"));
    }
    // A search of the web whose words look like a command is not one.
    call("q1", "WebSearch", "websearch", "cd /tmp && ls", "results");
    // Two shell steps in a row are one row, whose title is the first one's, then how many more.
    call("f1", "Bash", "bash", `cd ${SHOP} && git status`, "clean");
    call("f2", "Bash", "bash", `cd ${SHOP} && git log -1`, "abc123");
    await openConversation(page, turns);
    await installStepsProbe(page);
    const seen = await page.evaluate(async ({ ids, whole }) => {
      const { steps, lineOf, settle } = window.__STEPS__;
      await settle();
      const rowOf = (id) => steps().find((row) => row.__turn?.tool?.call_id === id);
      const targetOf = (row) => lineOf(row)?.querySelector(".helper-step-target")?.textContent ?? null;
      const seen = { fit: STEP_TARGET_FIT, titles: {}, kinds: {} };
      for (const id of ids) {
        seen.titles[id] = targetOf(rowOf(id));
        seen.kinds[id] = lineOf(rowOf(id))?.querySelector(".helper-step-kind")?.textContent ?? null;
      }
      seen.search = targetOf(rowOf("q1"));
      const run = steps().find((row) => row.classList.contains("is-run") && row.__kind === "bash");
      seen.run = targetOf(run);
      seen.runMore = t("worker.stepMore", "외 {{n}}개", { n: 1 });
      // The opened row keeps the whole command.
      lineOf(rowOf("s1")).click();
      await settle();
      seen.opened = (rowOf("s1")?.querySelector(":scope > .helper-step-body")?.innerText ?? "").replace(/\s+/g, " ");
      seen.whole = whole.replace(/\s+/g, " ");
      return seen;
    }, { ids: SHELL_TITLES.map((one) => one.id), whole: SHELL_TITLES[0].command });
    const head = (words) => (words ?? "").split(" · ")[0];
    const t1 = seen.titles.s1 ?? "";
    ok(
      "D1: a shell step whose command opens with `cd <dir> &&` is titled with the command's own first words, cut at the fit with a mark, and the folder's last name after 「 · 」 — the cd and the folder's whole path are not in the title",
      t1.startsWith('grep -rn "SCREEN_WIDTH') && t1.endsWith(" · shop-app") && head(t1).endsWith("…") &&
        head(t1).length <= seen.fit && !/(^|\s)cd\s/.test(t1) && !t1.includes("/Users/dev") && !t1.includes("&&"),
      JSON.stringify(seen),
    );
    ok(
      "D1: `cd \"a b\" ;` and `cd a && cd b &&` are read the same way — the semicolon's chain, a quoted folder with a space in it, the last folder named — and a command with no folder to name ends where the command does",
      seen.titles.s2 === SHELL_TITLES[1].title && seen.titles.s3 === SHELL_TITLES[2].title,
      JSON.stringify(seen.titles),
    );
    const t4 = seen.titles.s4 ?? "";
    ok(
      "D1: a shell command with no cd is fitted too — its own first words cut at the fit with a mark, no folder — and a `cd` inside the command's words is not stripped",
      t4.endsWith("…") && t4.length <= seen.fit && !t4.includes(" · ") &&
        SHELL_TITLES[3].command.startsWith(t4.slice(0, -1).trimEnd()) && seen.titles.s5 === SHELL_TITLES[4].title,
      JSON.stringify(seen.titles),
    );
    ok(
      "D1: only a shell step is read as a command — a search of the web whose query begins `cd /tmp &&` keeps its words — and a row of shell steps is titled like one step, then how many more stand behind it",
      seen.search === "cd /tmp && ls" && seen.run.includes("git status") && seen.run.includes(" · shop-app") &&
        !/(^|\s)cd\s/.test(seen.run) && seen.run.endsWith(seen.runMore),
      JSON.stringify({ search: seen.search, run: seen.run }),
    );
    ok(
      "D1: the opened row keeps the whole command, the cd and the folder's whole path included",
      seen.opened.includes(seen.whole) && seen.whole.includes(`cd ${SHOP} &&`),
      JSON.stringify({ opened: seen.opened.slice(0, 300), whole: seen.whole }),
    );
  } finally {
    await page.close();
  }
  // The foot line says the same title while the step is out.
  const live = await openWindowTestPage(browser, origin);
  try {
    await openConversation(live.page, [{ role: "user", text: "셸로 확인해 줘.", at_ms: CLOCK }], { status: "working" });
    await installStepsProbe(live.page);
    const saying = await live.page.evaluate(async ({ command }) => {
      const { list, settle } = window.__STEPS__;
      window.__CONVERSATION__.turns.push({
        role: "tool", text: `Bash · ${command}`, at_ms: 1_790_000_100_000,
        tool: { call_id: "live-shell", name: "Bash", kind: "bash", input: command, is_error: false },
      });
      await pollHelperPages();
      await settle();
      return {
        now: list().querySelector(":scope > .helper-status .helper-status-now")?.textContent ?? "",
        row: list().querySelector(":scope > .is-tool.is-live .helper-step-target")?.textContent ?? "",
      };
    }, { command: `cd ${SHOP} && make test-all-the-things --with --a-long --list --of --flags --that --goes --on` });
    ok(
      "D1: the foot line names a shell step that is out by its title — the command's first words and the folder — not by the whole `cd … &&` line",
      saying.now.endsWith(`${saying.row}`) && saying.row.endsWith(" · shop-app") && !/(^|\s)cd\s/.test(saying.now) &&
        saying.now.includes("make test-all-the-things"),
      JSON.stringify(saying),
    );
    ok("D1: the titles raised no page errors", live.faults.length === 0, live.faults.join("\n"));
  } finally {
    await live.page.close();
  }
}

/* D2 — a length only when the file said both ends. A line the file did not stamp is read at the moment
 * the window read it, so lines read in one batch share one time and a step between two of them "took"
 * 0.0 s — or, where only one end was stamped, the days between the file's clock and the window's. A
 * step shows its length when both of its ends carry a time from the file, and nothing otherwise; a
 * length that comes to nothing is nothing. The thought's own fold is worded 「생각 12초」. */
async function stepsUnknownTimes(browser, origin, ok) {
  const read = (page) => page.evaluate(async () => {
    const { list, steps, thoughts, lineOf, wordsOf, settle } = window.__STEPS__;
    await settle();
    return {
      metas: steps().map((row) => row.querySelector(":scope > summary .helper-step-meta")?.textContent ?? null),
      runs: steps().filter((row) => row.classList.contains("is-run")).length,
      text: list().innerText,
      thought: wordsOf(lineOf(thoughts()[0])),
    };
  });
  // The backend says `null` for a line it found no time on (`TranscriptTurn.at_ms`), not a missing key.
  const bare = (turns) => turns.map((turn) => ({ ...turn, at_ms: null }));
  // A file that stamped nothing: none of its steps says a length, and the page says no 「0.0초」.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await openConversation(page, bare(stepsFixture()));
      await installStepsProbe(page);
      const seen = await read(page);
      ok(
        "D2: a file that stamped nothing — its lines read in one batch, so they share the moment they were read — shows no length on any step or row of steps, and no 「0.0초」 anywhere on the page",
        seen.metas.length >= 5 && seen.runs >= 1 && seen.metas.every((meta) => meta === "") && !/\d\.\d초/.test(seen.text),
        JSON.stringify(seen.metas),
      );
      ok("D2: the unstamped page raised no page errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
  // A file that stamped everything: every step says its own length, and a thought says how long it thought.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await openConversation(page, stepsFixture());
      await installStepsProbe(page);
      const seen = await read(page);
      ok(
        "D2: stamped steps show theirs — one decimal and 「초」, the row of steps from its first call to its last answer",
        seen.metas.length >= 5 && seen.metas.every((meta) => /^\d+\.\d초$/.test(meta ?? "")),
        JSON.stringify(seen.metas),
      );
      ok(
        "D2: a thought that lasted 12 seconds is worded 「생각 12초」 — the word first, then the length — not 「12초 동안 생각」",
        seen.thought.includes("생각 12초") && !seen.thought.includes("동안"),
        seen.thought,
      );
      ok("D2: the stamped page raised no page errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
  // Half a stamp is no stamp: the call's or the answer's missing, or the two the same.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      const turns = [{ role: "user", text: "시간을 알 수 없는 줄들.", at_ms: CLOCK }];
      const pair = (id, name, kind, target, call, answer) => {
        turns.push({ role: "tool", text: `${name} · ${target}`, ...call, tool: { call_id: id, name, kind, input: target, is_error: false } });
        turns.push({ role: "tool_result", text: "ok", ...answer, tool: { call_id: id, is_error: false } });
      };
      // Four kinds, so no two of them fold into one row.
      pair("m1", "WebFetch", "web", "https://example.com/a", { at_ms: CLOCK + 1000 }, { at_ms: CLOCK + 3000 });
      pair("m2", "Read", "read", `${PROFILE}/A.tsx`, { at_ms: CLOCK + 4000 }, { at_ms: null });
      pair("m3", "Grep", "grep", "SCREEN_WIDTH", { at_ms: null }, { at_ms: CLOCK + 6000 });
      pair("m4", "Bash", "bash", "ls", { at_ms: CLOCK + 7000 }, { at_ms: CLOCK + 7000 });
      // A stamp that goes backwards is no length either — a session after compaction opens with a summary
      // stamped at the moment it was written, later than the older lines that follow it (by design, t-18703),
      // so the difference of two stamps is not always a time: a step whose answer is stamped before its call,
      // and a row of steps whose last answer is stamped before its first call, say nothing.
      pair("m5", "Edit", "edit", `${PROFILE}/A.tsx`, { at_ms: CLOCK + 9000 }, { at_ms: CLOCK + 8000 });
      pair("m6", "Read", "read", `${PROFILE}/B.tsx`, { at_ms: CLOCK + 12_000 }, { at_ms: CLOCK + 12_300 });
      pair("m7", "Read", "read", `${PROFILE}/C.tsx`, { at_ms: CLOCK + 11_000 }, { at_ms: CLOCK + 11_200 });
      await openConversation(page, turns);
      await installStepsProbe(page);
      const seen = await read(page);
      ok(
        "D2: a step whose answer was not stamped, one whose call was not, one whose two ends are the same instant, one whose answer is stamped before its call, and a row of steps whose last answer is stamped before its first call say no length — only the step both of whose ends the file stamped, in order, says its 2.0 seconds",
        JSON.stringify(seen.metas) === JSON.stringify(["2.0초", "", "", "", "", ""]) && seen.runs === 1 &&
          !/\d\.\d초|-\d/.test(seen.text.replace("2.0초", "")),
        JSON.stringify(seen.metas),
      );
      ok("D2: the half-stamped page raised no page errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
}

/* D3 — the header clock is the helper's, and says nothing when it cannot know. The head read 「실행 중
 * 0초」 (counted from the moment the page opened) and 「완료 0초」 on every finished helper, whichever
 * hour it ran. The helper's own clock is the span of the times its file stamped — its first line to its
 * last, or to now while it runs — and a file that stamped nothing has no clock. A helper that ended
 * reads 「✓ 끝남 · 2분 14초」 (the mockup's chip): the word, a check before it, the length after a dot. */
async function stepsHelperClock(browser, origin, ok) {
  const clockOf = (page) => page.evaluate(async () => {
    await window.__PAINTED__();
    await window.__PAINTED__();
    const head = document.querySelector("#worker-view .helper-head");
    const state = head?.querySelector(".worker-state");
    const clock = head?.querySelector(".worker-elapsed");
    const before = (node) => (node ? getComputedStyle(node, "::before").content : "");
    return {
      state: state?.textContent ?? null,
      clock: clock?.textContent ?? null,
      check: before(state),
      dot: before(clock),
      ended: t("worker.ended", "끝남"),
    };
  });
  const stamped = (span, { lines = 2 } = {}) => {
    const turns = [{ role: "user", text: "이 도우미에게 맡긴 일.", at_ms: CLOCK }];
    for (let at = 1; at < lines; at += 1) {
      turns.push({
        role: "tool", text: `Read · ${PROFILE}/A${at}.tsx`, at_ms: CLOCK + Math.round((span * at) / lines),
        tool: { call_id: `h${at}`, name: "Read", kind: "read", input: `${PROFILE}/A${at}.tsx`, is_error: false },
      });
      turns.push({
        role: "tool_result", text: "ok", at_ms: CLOCK + Math.round((span * at) / lines) + 100, tool: { call_id: `h${at}`, is_error: false },
      });
    }
    turns.push({ role: "assistant", text: "끝났습니다.", at_ms: CLOCK + span });
    return turns;
  };
  // Ended: from the first stamped line to the last — the mockup's 2 minutes 14 seconds.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await openHelperConversation(page, stamped(134_000, { lines: 4 }), "done");
      const seen = await clockOf(page);
      ok(
        "D3: a helper that ended says how long it ran by its file's own stamps — 「2분 14초」 for a file whose first line and last are 134 seconds apart — however long ago it ran and whenever its page was opened",
        seen.clock === "2분 14초",
        JSON.stringify(seen),
      );
      ok(
        "D3: a helper that ended wears 「끝남」 with a check before it and its length after a dot — the mockup's 「✓ 끝남 · 2분 14초」",
        seen.state === seen.ended && seen.state === "끝남" && seen.check.includes("✓") && seen.dot.includes("·"),
        JSON.stringify(seen),
      );
      ok("D3: the ended helper's head raised no page errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
  // Running: from the first stamped line to now, counting on.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      const start = Date.now() - 41_000;
      const turns = [
        { role: "user", text: "지금 돌고 있는 도우미.", at_ms: start },
        { role: "tool", text: `WebFetch · https://example.com/health`, at_ms: start + 1000,
          tool: { call_id: "run1", name: "WebFetch", kind: "web", input: "https://example.com/health", is_error: false } },
        { role: "tool_result", text: "200 OK", at_ms: start + 2000, tool: { call_id: "run1", is_error: false } },
      ];
      await openHelperConversation(page, turns, "running");
      const seen = await clockOf(page);
      ok(
        "D3: a helper that is running counts from its file's first stamped line, not from the moment the page opened — 41 seconds in, the head says a number of seconds in the forties or fifties, and no 「✓」 stands before 「실행 중」",
        /^(4|5)\d초$/.test(seen.clock ?? "") && !seen.check.includes("✓") && seen.dot.includes("·"),
        JSON.stringify(seen),
      );
      ok("D3: the running helper's head raised no page errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
  // A session after compaction opens with a summary stamped at the moment it was written — later than the
  // older lines that follow it (by design, t-18703): a clock that would run backwards is no clock, and a
  // number measured from the summary to the newer lines would be a time since the compaction, not the helper's.
  for (const [name, more] of [["only the older lines follow", []], ["older lines and newer ones follow", [{ role: "assistant", text: "요약 뒤에 이어 한 일.", at_ms: CLOCK + 620_000 }]]]) {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      const turns = [{ role: "user", text: "이전 대화의 요약.", at_ms: CLOCK + 600_000 }, ...stamped(134_000, { lines: 4 }), ...more];
      await openHelperConversation(page, turns, "done");
      const seen = await clockOf(page);
      ok(
        `D3: a helper file that opens with a summary stamped later than the lines after it (${name}) gives the head no clock — no backwards span, no time since the summary`,
        seen.clock === "" && seen.state === seen.ended && !seen.dot.includes("·"),
        JSON.stringify(seen),
      );
      ok(`D3: the compacted helper's head (${name}) raised no page errors`, faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
  // Unknown: a file that stamped nothing has no clock — no number, no dot, and not 「0초」.
  {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      const bare = stamped(134_000, { lines: 4 }).map((turn) => ({ ...turn, at_ms: null }));
      await openHelperConversation(page, bare, "done");
      const seen = await clockOf(page);
      ok(
        "D3: a file that stamped nothing gives the head no clock at all — the state word alone, no 「0초」, no dot after it",
        seen.clock === "" && seen.state === seen.ended && !seen.dot.includes("·"),
        JSON.stringify(seen),
      );
      ok("D3: the unstamped helper's head raised no page errors", faults.length === 0, faults.join("\n"));
    } finally {
      await page.close();
    }
  }
}

/* D4 — the now line, when no row of a step or a thought is out to name. A helper's page reads its own
 * card's newest activity (zo's `subagents` frame names what each helper is doing, and the window files
 * it under `sub:<term>:<id>`); a tool's verb is worded the way the step rows word that kind, waiting,
 * reconnecting and thinking are the page's own words, and when nothing is known the line says the
 * window's own 「작업 중…」 — never the agent's English word on a Korean page. An agent whose voice turns
 * through verbs (Claude Code's) keeps its own line where nothing is known. */
async function stepsNowWithoutARow(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await standZo(page);
    // The helper is still at work, and every call it made has come back.
    await openHelperConversation(page, stepsFixture().filter((turn) => turn.role !== "assistant"), "running", { agent: "zo" });
    await installStepsProbe(page);
    const seen = await page.evaluate(async () => {
      const { settle } = window.__STEPS__;
      const tab = activeHelperPage();
      const card = `sub:${tab.worker.term}:${tab.worker.helper.id}`;
      const status = () => document.querySelector("#worker-view .helper-status");
      const nowWords = () => status()?.querySelector(".helper-status-now")?.textContent ?? "";
      const seeing = () => (status()?.innerText ?? "").replace(/\s+/g, " ").trim();
      const state = () => ({ now: nowWords(), seeing: seeing(), naming: status()?.classList.contains("is-naming") ?? false });
      const tell = (name, payload) => {
        for (const handler of window.__LISTENERS__[name] ?? []) handler({ payload });
      };
      let seq = 100;
      const say = async (activity) => {
        tell("hook:activity", { pane: card, activities: [{ seq: (seq += 1), activity }] });
        await settle();
        return state();
      };
      const feed = async (...turns) => {
        window.__HELPER_TURNS__.push(...turns);
        await pollHelperPages();
        await settle();
      };
      const voice = agentVoice("zo");
      const now = t("worker.now", "지금");
      const seen = {
        busy: voice.busy_word,
        wordNode: status()?.querySelector(".helper-status-word")?.textContent ?? "",
        wantNothing: `${now} · ${t("worker.busy", "작업 중…")}`,
        wantWaiting: `${now} · ${t("worker.nowWaiting", "답을 기다리는 중")}`,
        wantReconnecting: `${now} · ${t("worker.nowReconnecting", "다시 연결하는 중")}`,
        wantThinking: `${now} · ${t("worker.nowThinking", "생각하는 중")}`,
      };
      await settle();
      seen.nothing = state();
      // What a call's own row says on the foot line while it is out, and what the card's activity says
      // of the same call when the row is gone: one wording.
      const same = {};
      const shop = "/Users/dev/shop-app";
      let call = 0;
      const wear = async (key, name, kind, target, activity) => {
        // The card's newest activity is one that is over, so it names nothing while the call's row is out.
        await say({ verb: "read", target: "x", phase: "finished" });
        call += 1;
        const id = `same-${call}`;
        const turn = { role: "tool", text: `${name} · ${target}`, at_ms: 1_790_000_200_000 + call * 10_000,
          tool: { call_id: id, name, ...(kind !== null && { kind }), input: target, is_error: false } };
        await feed(turn);
        const row = nowWords();
        await feed({ role: "tool_result", text: "ok", at_ms: turn.at_ms + 900, tool: { call_id: id, is_error: false } });
        const after = state();
        const said = await say(activity);
        same[key] = { row, after: after.now, card: said.now };
      };
      await wear("web", "WebFetch", "web", "https://example.com/health", { verb: "web", target: "https://example.com/health", phase: "started" });
      await wear("bash", "Bash", "bash", `cd ${shop} && make test`, { verb: "bash", target: `cd ${shop} && make test`, phase: "started" });
      await wear("read", "Read", "read", `${shop}/src/screens/profile/Footer.tsx`, { verb: "read", target: `${shop}/src/screens/profile/Footer.tsx`, phase: "started" });
      await wear("other", "mcp__shop__lookup", null, "sku-1", { verb: "mcp__shop__lookup", target: "sku-1", phase: "started" });
      seen.same = same;
      seen.waiting = await say({ verb: "waiting", target: "model", phase: "started" });
      seen.reconnecting = await say({ verb: "reconnecting", target: "attempt 2 in 5s", phase: "started" });
      seen.thinking = await say({ verb: "reasoning silently", phase: "started" });
      seen.quiet = await say({ verb: "quiet", target: "last: read a.rs", phase: "started" });
      seen.finished = await say({ verb: "read", target: "a.rs", phase: "finished" });
      // A call still out is named again, and the turn's end takes the line away.
      seen.again = await say({ verb: "grep", target: "SCREEN_WIDTH", phase: "started" });
      tell("hook:subagent", { term: tab.worker.term, rows: [{ id: tab.worker.helper.id, name: "프로필 도우미", state: "done" }] });
      await settle();
      seen.ended = { hidden: status()?.hidden === true };
      return seen;
    });
    const nothingKnown = (one) => one.now === seen.wantNothing && one.naming && !one.seeing.includes(seen.busy);
    ok(
      "D4: a helper page with no row out and nothing known says the window's own 「지금 · 작업 중…」 — the agent's English word (zo's 「Working…」) is not on the page, though the word's own node still holds it",
      seen.busy !== "" && nothingKnown(seen.nothing) && seen.wordNode === seen.busy,
      JSON.stringify(seen.nothing),
    );
    ok(
      "D4: what the helper's own card says it is doing — a page read, a shell command with its folder, a file read, a tool the page has no word for — is said in the words the step rows wear for that call, the row's own words and no second wording",
      ["web", "bash", "read", "other"].every((key) => seen.same[key]?.row !== "" && seen.same[key]?.card === seen.same[key]?.row &&
        seen.same[key]?.after === seen.wantNothing),
      JSON.stringify(seen.same),
    );
    ok(
      "D4: waiting, reconnecting and thinking are the page's words, one line each — and a card that says quiet, or a call that is already finished, says nothing the page can name, so the line is the window's 「작업 중…」 again",
      seen.waiting.now === seen.wantWaiting && seen.reconnecting.now === seen.wantReconnecting &&
        seen.thinking.now === seen.wantThinking && nothingKnown(seen.quiet) && nothingKnown(seen.finished),
      JSON.stringify({ w: seen.waiting.now, r: seen.reconnecting.now, t: seen.thinking.now, q: seen.quiet.now, f: seen.finished.now }),
    );
    ok(
      "D4: none of them says the agent's English word aloud, and every one is the naming band, not the agent's mark and word",
      [seen.waiting, seen.reconnecting, seen.thinking, seen.again].every((one) => one.naming && !one.seeing.includes(seen.busy)) &&
        seen.again.now.includes("SCREEN_WIDTH"),
      JSON.stringify(seen.again),
    );
    ok("D4: when the helper's turn ends the line goes with it", seen.ended.hidden, JSON.stringify(seen.ended));
    ok("D4: the now line raised no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  // An agent whose voice turns through verbs keeps its own line where nothing is known.
  const claude = await openWindowTestPage(browser, origin);
  try {
    await openHelperConversation(claude.page, stepsFixture().filter((turn) => turn.role !== "assistant"), "running");
    await installStepsProbe(claude.page);
    const kept = await claude.page.evaluate(async () => {
      await window.__STEPS__.settle();
      const status = document.querySelector("#worker-view .helper-status");
      return {
        naming: status?.classList.contains("is-naming") ?? true,
        now: status?.querySelector(".helper-status-now")?.textContent ?? null,
        verbs: agentVoice("claude").spinner_verbs.length,
      };
    });
    ok(
      "D4: an agent whose voice turns through verbs (Claude Code's) keeps its own line where nothing is known — the fallback is for a voice with one static word",
      kept.verbs > 0 && kept.naming === false && kept.now === "",
      JSON.stringify(kept),
    );
    ok("D4: the kept voice raised no page errors", claude.faults.length === 0, claude.faults.join("\n"));
  } finally {
    await claude.page.close();
  }
}

/* D5 — a list written right under a sentence is a list. A paragraph ran until a blank line or a line
 * that opens a block, and a list item was not one, so 「… 찾았습니다.\n- 원인: …\n- 새 규칙: …」 ran
 * together as one paragraph. CommonMark's rule: a bullet (`-`, `*`, `+` and a space, with words after
 * it) interrupts a paragraph; an ordered item does only when it starts at 1 — 「2024. 한 해」 under a
 * sentence stays text, and so does a hyphen that opens a word, an item with nothing in it, and a line
 * indented four spaces (the paragraph's own continuation). */
const READER_ROWS = [
  ["a bullet under a sentence", "문장입니다.\n- a\n- b", { paras: 1, lists: ["ul"], items: 2 }],
  ["a star under a sentence", "문장입니다.\n* a\n* b", { paras: 1, lists: ["ul"], items: 2 }],
  ["a plus under a sentence", "문장입니다.\n+ a", { paras: 1, lists: ["ul"], items: 1 }],
  ["an ordered list from 1", "문장입니다.\n1. a\n2. b", { paras: 1, lists: ["ol"], items: 2 }],
  ["an ordered list from 1 with a parenthesis", "문장입니다.\n1) a\n2) b", { paras: 1, lists: ["ol"], items: 2 }],
  ["a bullet indented two spaces", "문장입니다.\n  - a", { paras: 1, lists: ["ul"], items: 1 }],
  ["a sentence, a blank line and a list (as ever)", "문장입니다.\n\n- a\n- b", { paras: 1, lists: ["ul"], items: 2 }],
  ["a list, a blank line and a sentence", "- a\n- b\n\n문장입니다.", { paras: 1, lists: ["ul"], items: 2 }],
  // The line after an item is that item's own, and the item that breaks it off is the same list's next one.
  ["an item, its continuation line and the next item", "- a\n  continued\n- b", { paras: 0, lists: ["ul"], items: 2 }],
  ["a year with a stop", "문장입니다.\n2024. 한 해가 저물었다.", { paras: 1, lists: [], items: 0 }],
  ["an ordered item that starts at 2", "문장입니다.\n2. b\n3. c", { paras: 1, lists: [], items: 0 }],
  ["a hyphen that opens a word", "문장입니다.\n-하이픈으로 시작하는 낱말", { paras: 1, lists: [], items: 0 }],
  ["a bullet with nothing in it", "문장입니다.\n- ", { paras: 1, lists: [], items: 0 }],
  ["a bullet indented four spaces", "문장입니다.\n    - a", { paras: 1, lists: [], items: 0 }],
  ["a dash in the middle of a line", "문장입니다 - a - b", { paras: 1, lists: [], items: 0 }],
];

async function stepsListUnderASentence(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const seen = await page.evaluate((rows) => rows.map(([name, text]) => {
      const host = document.createElement("div");
      paintHelperProse(host, text, "/tmp/zerocode-window-test");
      return {
        name,
        paras: host.querySelectorAll(":scope > p.md-para").length,
        lists: [...host.children].filter((node) => node.matches("ul.md-list, ol.md-list")).map((node) => node.tagName.toLowerCase()),
        items: host.querySelectorAll("li").length,
        text: host.innerText.replace(/\s+/g, " ").trim(),
      };
    }), READER_ROWS);
    const wrong = READER_ROWS.filter(([, , want], at) => JSON.stringify({ paras: seen[at].paras, lists: seen[at].lists, items: seen[at].items }) !== JSON.stringify(want))
      .map(([name]) => name);
    ok(
      "D5: a bullet, a star or a plus, and an ordered item that starts at 1, written right under a sentence, is a list after a paragraph — and a year with a stop, an item that starts at 2, a hyphen that opens a word, a bullet with nothing in it and a line indented four spaces stay the sentence's own text",
      wrong.length === 0,
      JSON.stringify({ wrong, seen: seen.filter((one) => wrong.includes(one.name)) }),
    );
  } finally {
    await page.close();
  }
  // A helper's report, as a helper writes it.
  const report = await openWindowTestPage(browser, origin);
  try {
    const answer = "원인과 새 규칙을 찾았습니다.\n- 원인: 셀의 key가 인덱스여서 스크롤할 때마다 모든 셀이 다시 만들어집니다.\n- 새 규칙: key는 id로 씁니다.";
    await openHelperConversation(report.page, [
      { role: "user", text: "원인을 찾아 줘.", at_ms: CLOCK },
      { role: "assistant", text: answer, at_ms: CLOCK + 5000 },
    ], "done");
    const shown = await report.page.evaluate(async () => {
      await window.__PAINTED__();
      const said = document.querySelector("#worker-view .helper-report .helper-said");
      return {
        paras: said?.querySelectorAll(":scope > p.md-para").length ?? -1,
        lists: said?.querySelectorAll(":scope > ul.md-list").length ?? -1,
        items: [...(said?.querySelectorAll("li") ?? [])].map((item) => item.textContent),
      };
    });
    ok(
      "D5: a finished helper's report that writes its list right under its sentence stands as one paragraph and a list of two — 「원인: …」 and 「새 규칙: …」 are two lines, not the sentence's tail",
      shown.paras === 1 && shown.lists === 1 && shown.items.length === 2 && shown.items[0].startsWith("원인:") && shown.items[1].startsWith("새 규칙:"),
      JSON.stringify(shown),
    );
    ok("D5: the report raised no page errors", report.faults.length === 0, report.faults.join("\n"));
  } finally {
    await report.page.close();
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
    testConversationCopies, testConversationShelf, testConversationRelease, testConversationSteps,
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
