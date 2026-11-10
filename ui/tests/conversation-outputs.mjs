/* What an answer is made of, part by part — the third stage of the conversation
 * the person passed on 2026-11-10 (window artifact p-1c1bbf75726ff227, 「출력
 * 형태」): words nobody typed are a thin line, never the person's bubble. Each
 * check reads the page a browser laid out. The conversations are synthetic.
 *
 *   WINDOW_SUITES='^conversation-outputs' node ui/tests/window.mjs */
import { openAfterScene } from "./conversation-after.mjs";
import { openConversation, openPaneConversation, standZo } from "./conversation-parity.mjs";
import { foldConversations, openWindowTestPage } from "./window-boot.mjs";

/* A pane's conversation that has ended, on the folded page. */
async function openSettled(page, turns) {
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
}

/* The lines a CLI writes for the model in the person's place (`skill_note`, `hook_note`, `cli_note` —
 * named by the transcript reader): a skill's body folds with the work that read it, a hook's feedback and
 * another session's message are thin lines, and none of them is a bubble or ends the person's turn. */
export async function testConversationCliNotes(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const body = `Base directory for this skill: /Users/dev/.claude/skills/artifact-design\n\n# 완성물을 페이지로\n\n${
      Array.from({ length: 80 }, (_, at) => `- 지킬 것 ${at + 1}: 목적이 구성을 정한다.`).join("\n")}`;
    await openSettled(page, [
      { role: "user", text: "스킬로 페이지를 만들어 줘." },
      { role: "tool", text: "Skill artifact-design", tool: { call_id: "s1", name: "Skill", kind: "other", input: "artifact-design" } },
      { role: "tool_result", text: "Launching skill: artifact-design", tool: { call_id: "s1", is_error: false } },
      { role: "skill_note", text: body },
      { role: "tool", text: "Write page.html", tool: { call_id: "w1", name: "Write", kind: "edit", input: "page.html", file: { path: "page.html" },
        edits: [{ path: "page.html", lines: [{ kind: "add", text: "<!doctype html>" }] }] } },
      { role: "tool_result", text: "created", tool: { call_id: "w1", is_error: false } },
      { role: "assistant", text: "페이지를 만들었습니다." },
      { role: "hook_note", text: "Stop hook feedback:\nyou have 1 message" },
      { role: "assistant", text: "편지를 확인했습니다." },
      { role: "cli_note", text: "Another Claude session sent a message:\nrebase first, then run the window suite again" },
      { role: "assistant", text: "main 위로 올렸습니다." },
    ]);
    const read = () => page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const rows = [...list.querySelectorAll(":scope > [data-turn]")];
      const kind = (row) => row.classList.contains("is-user") ? "person"
        : row.classList.contains("helper-group") ? (row.classList.contains("is-turn-fold") ? "fold" : "pill")
        : row.classList.contains("is-window-note") ? `note:${row.__turn.role}`
        : row.classList.contains("is-assistant") ? "answer" : row.className;
      const fold = rows.find((row) => row.classList.contains("helper-group"));
      const notes = rows.filter((row) => row.classList.contains("is-window-note"));
      const answers = rows.filter((row) => row.classList.contains("is-assistant"));
      return {
        rows: rows.map(kind),
        bubbles: list.querySelectorAll(".helper-turn.is-user").length,
        foldMembers: fold?.__members.map((turn) => turn.role) ?? [],
        foldWords: fold?.querySelector(".helper-group-words").textContent ?? null,
        notes: notes.map((row) => row.querySelector(".helper-window-note-words").textContent),
        noteIsCli: notes.map((row) => row.classList.contains("is-cli-note")),
        finalAnswers: answers.filter((row) => row.classList.contains("is-turn-answer")).map((row) => row.querySelector(".helper-said").textContent),
        card: list.querySelector(".helper-turn.is-latest > .chat-result .chat-result-title")?.textContent ?? null,
        cardPath: list.querySelector(".helper-turn.is-latest > .chat-result .chat-result-path")?.textContent ?? null,
      };
    });
    const start = await read();
    ok("notes: a skill's body, a hook's feedback and another session's message are no bubbles — the page holds the one thing the person said",
      start.bubbles === 1 && start.rows.join(" ") === "person fold answer note:hook_note answer note:cli_note answer",
      JSON.stringify(start));
    ok("notes: the skill's body folds with the work that read it — the turn is one line, not two with a body between them",
      start.foldMembers.join(" ") === "tool skill_note tool" && start.foldWords.includes("1개"), JSON.stringify(start));
    ok("notes: a hook's feedback says its event and a CLI's own line its first words, each with the door to the whole of it",
      start.notes.join(" | ") === "훅 알림 · Stop · 원문 | CLI 알림 · Another Claude session sent a message · 원문" && start.noteIsCli.every(Boolean),
      JSON.stringify(start.notes));
    ok("notes: none of them ends the person's turn — its one final answer is the last, and the card under it counts the file the turn wrote",
      start.finalAnswers.join("|") === "main 위로 올렸습니다." && start.card === "변경된 파일 1개" && start.cardPath === "page.html",
      JSON.stringify(start));

    // The fold opens to its steps, the skill's note among them; the note opens to the body, which scrolls in its own box.
    const opened = await page.evaluate(async () => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const fold = list.querySelector(":scope > .helper-group");
      fold.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      const note = fold.querySelector(":scope > .helper-group-body > .is-window-note");
      const words = note?.querySelector(".helper-window-note-words").textContent ?? null;
      note?.querySelector("summary").click();
      await window.__PAINTED__();
      const said = note?.querySelector(".helper-window-note-said");
      return {
        words, lang: said?.lang ?? null, opens: said?.textContent.startsWith("Base directory for this skill:") ?? false,
        holds: said?.textContent.includes("지킬 것 80") ?? false,
        scrolls: said ? said.scrollHeight > said.clientHeight + 1 && getComputedStyle(said).overflowY === "auto" : false,
        height: said ? Math.round(said.getBoundingClientRect().height) : 0, screen: window.innerHeight,
        sideways: list.scrollWidth - list.clientWidth,
      };
    });
    ok("notes: opened, the turn's line holds 「스킬을 읽음 · artifact-design」 among its steps, and that opens to the whole body in a box of its own — no taller than most of the screen, in the skill's own language",
      opened.words === "스킬을 읽음 · artifact-design · 원문" && opened.opens && opened.holds && opened.scrolls &&
        opened.height <= opened.screen * 0.6 + 1 && opened.lang === "" && opened.sideways <= 0,
      JSON.stringify(opened));

    // A CLI's line that is only plumbing says what it can, cut to one line.
    const heads = await page.evaluate(() => [
      cliNoteHead("<local-command-caveat>The command below was run by the person</local-command-caveat>"),
      cliNoteHead(`\n\n${"long ".repeat(40)}`),
      windowNoteWords({ role: "skill_note", text: "Base directory for this skill: C:\\Users\\dev\\.claude\\skills\\design" })[0],
    ]);
    ok("notes: a line's tags are not its words, a long first line is cut with an ellipsis, and a skill's folder is read on Windows too",
      heads[0] === "The command below was run by the person" && heads[1].length === 72 && heads[1].endsWith("…") &&
        heads[2] === "스킬을 읽음 · design",
      JSON.stringify(heads));
    ok("notes: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A fold that failed says why without being opened: the first step that failed and the first line it
 * printed stand under its line; the step's own row says the same line; opened, the fold's line steps aside. */
export async function testConversationFailedFold(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openSettled(page, [
      { role: "user", text: "고치고 테스트까지 돌려 줘." },
      { role: "tool", text: "Edit src/session.rs", tool: { call_id: "e1", name: "Edit", kind: "edit", input: "src/session.rs", file: { path: "src/session.rs" },
        edits: [{ path: "src/session.rs", lines: [{ kind: "add", text: "let head = read_head(id);" }] }] } },
      { role: "tool_result", text: "updated", tool: { call_id: "e1", is_error: false } },
      { role: "tool", text: "Bash cargo test -p session", tool: { call_id: "t1", name: "Bash", kind: "bash", input: "cargo test -p session" } },
      { role: "tool_result", text: "Exit code 101\nerror[E0425]: cannot find function `read_head` in this scope\n --> src/session.rs:12:16", tool: { call_id: "t1", is_error: true } },
      { role: "tool", text: "Bash cargo fmt", tool: { call_id: "t2", name: "Bash", kind: "bash", input: "cargo fmt" } },
      { role: "tool_result", text: "Exit code 1", tool: { call_id: "t2", is_error: true } },
      { role: "assistant", text: "테스트가 실패했습니다. `read_head`를 아직 만들지 않았습니다." },
      { role: "user", text: "그럼 읽기만 해 줘." },
      { role: "tool", text: "Read src/session.rs", tool: { call_id: "r1", name: "Read", kind: "read", input: "src/session.rs", file: { path: "src/session.rs" } } },
      { role: "tool_result", text: "fn open_session() {}", tool: { call_id: "r1", is_error: false } },
      { role: "assistant", text: "읽었습니다." },
    ]);
    const read = () => page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      return [...list.querySelectorAll(":scope > .helper-group")].map((fold) => {
        // The fold's own generated line: what it says is the fold's `data-why`, drawn by `::after`.
        const style = getComputedStyle(fold, "::after");
        const drawn = style.content !== "none" && style.display !== "none";
        return {
          failed: fold.classList.contains("is-failed"), words: fold.querySelector(".helper-group-words").textContent,
          mark: getComputedStyle(fold.querySelector(".helper-group-cap"), "::before").content,
          why: fold.dataset.why ?? null, shown: drawn,
          draws: style.content !== "none" && style.content.includes("read_head"),
          oneLine: drawn ? style.whiteSpace === "nowrap" && style.textOverflow === "ellipsis" : null,
          nodes: fold.querySelectorAll(".helper-group-why").length,
          taller: Math.round(fold.getBoundingClientRect().height - fold.querySelector(".helper-group-cap").getBoundingClientRect().height),
          sideways: list.scrollWidth - list.clientWidth,
        };
      });
    });
    const [broke, whole] = await read();
    ok("failed: a fold with a failure says which step failed and the first line it printed — past a bare exit code — under its own line, one line inside the column, and pays no node for it",
      broke.failed && broke.words.includes("실패 2") && broke.mark === '"✕"' && broke.shown && broke.draws && broke.oneLine && broke.nodes === 0 &&
        broke.taller >= 12 && broke.sideways <= 0 &&
        broke.why === "명령 cargo test -p session — error[E0425]: cannot find function `read_head` in this scope",
      JSON.stringify(broke));
    ok("failed: a fold whose work came back whole has no such line", !whole.failed && whole.why === null && !whole.shown && whole.taller <= 9 && whole.mark === '"✓"',
      JSON.stringify(whole));

    const opened = await page.evaluate(async () => {
      const fold = document.querySelector(".pane-chat .helper-turns > .helper-group");
      fold.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      const rows = [...fold.querySelectorAll(":scope > .helper-group-body > .is-tool")];
      return {
        whyShown: getComputedStyle(fold, "::after").display !== "none",
        results: rows.map((row) => row.querySelector(".helper-step-res").textContent),
      };
    });
    ok("failed: opened, the fold's rows say it — each failed step with the same first line, a bare exit code when that is all it printed — and the line under the fold steps aside",
      !opened.whyShown && opened.results[1] === "실패: error[E0425]: cannot find function `read_head` in this scope" && opened.results[2] === "실패: Exit code 1",
      JSON.stringify(opened));
    const closed = await page.evaluate(async () => {
      const fold = document.querySelector(".pane-chat .helper-turns > .helper-group");
      fold.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      return getComputedStyle(fold, "::after").display !== "none" && fold.dataset.why !== undefined;
    });
    ok("failed: closed again, the line is back", closed === true);
    ok("failed: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A table in an answer: a frame its own cells draw, a ground under its head, a wide one scrolling inside its
 * own box — and one mark at the end of the head row that takes it away as markdown or as CSV. */
export async function testConversationTable(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const small = [
      "| 대화 크기 | 지금 | 고친 뒤 |",
      "| :-- | --: | :-: |",
      "| 400턴 | `210 ms` | **38 ms** |",
      "| 4,000턴, \"큰\" 대화 | 1,840 ms | 41 ms |",
    ].join("\n");
    const columns = Array.from({ length: 14 }, (_, at) => `넓은 칸 ${at + 1}`);
    const wide = [`| ${columns.join(" | ")} |`, `| ${columns.map(() => "---").join(" | ")} |`, `| ${columns.map((_, at) => `값이 긴 칸 ${at + 1}`).join(" | ")} |`].join("\n");
    await openSettled(page, [
      { role: "user", text: "얼마나 빨라졌는지 표로 보여 줘." },
      { role: "assistant", text: `잰 값입니다.\n\n${small}\n\n넓은 표도 하나 있습니다.\n\n${wide}\n\n끝입니다.` },
    ]);
    const frame = await page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const [table, broad] = list.querySelectorAll(".helper-turn.is-assistant .helper-said .md-table");
      const probe = (name) => {
        const span = document.createElement("span");
        span.style.color = `var(${name})`;
        list.appendChild(span);
        const value = getComputedStyle(span).color;
        span.remove();
        return value;
      };
      const edge = (cell) => {
        const style = getComputedStyle(cell);
        return { top: style.borderTopWidth, right: style.borderRightWidth, bottom: style.borderBottomWidth, left: style.borderLeftWidth, ink: style.borderTopColor };
      };
      const rows = [...table.rows];
      const box = (node) => node.getBoundingClientRect();
      // The frame `dressTables` stands a settled table in: the table and, beside it, its copy.
      const frame = table.parentElement;
      const copy = frame.querySelector(":scope > .helper-table-copy");
      const broadCopy = broad.parentElement.querySelector(":scope > .helper-table-copy");
      const prose = table.closest(".helper-said").querySelector(".md-para");
      return {
        divider: probe("--chat-divider"),
        headFirst: edge(rows[0].cells[0]), headMid: edge(rows[0].cells[1]), headLast: edge(rows[0].cells[2]),
        midFirst: edge(rows[1].cells[0]), footMid: edge(rows[2].cells[1]),
        corner: getComputedStyle(rows[0].cells[0]).borderTopLeftRadius, footCorner: getComputedStyle(rows[2].cells[2]).borderBottomRightRadius,
        headGround: getComputedStyle(rows[0].cells[0]).backgroundColor, bodyGround: getComputedStyle(rows[1].cells[0]).backgroundColor,
        hugs: Math.round(box(rows[0].cells[2]).right) < Math.round(box(prose).right) - 40,
        broadScrolls: broad.scrollWidth > broad.clientWidth + 1, broadInside: Math.round(box(broad).right) <= Math.round(box(list).right),
        sideways: list.scrollWidth - list.clientWidth, pageSideways: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        copies: list.querySelectorAll(".helper-table-copy").length,
        copyBeside: frame.classList.contains("helper-table") && copy?.parentElement === frame && broadCopy?.parentElement === broad.parentElement,
        headButtons: table.querySelectorAll("button").length + broad.querySelectorAll("button").length,
        copyName: copy?.getAttribute("aria-label"), copyTip: copy?.dataset.tip, copyWords: copy?.textContent, copyAtRest: copy ? getComputedStyle(copy).opacity : null,
        // Outside the grid, on the head row's band: no cell's words are under it.
        copyClear: copy ? box(copy).left >= box(rows[0].cells[2]).right && box(copy).top >= box(rows[0]).top - 1 && box(copy).bottom <= box(rows[0]).bottom + 1 : null,
        // Beside the table that scrolls inside its own box too: in view with nothing scrolled, inside the column.
        broadCopyInView: broadCopy ? box(broadCopy).left >= box(broad).right && box(broadCopy).right <= box(prose).right + 1 : null,
        // The room beside a table is kept from the frame it is written in: a bare table is never wider than a dressed one.
        broadRoom: Math.round(box(prose).right - box(broad).right),
      };
    });
    const one = "1px";
    ok("table: a frame the cells draw — a rule over each row, the two sides on the first and last cell, the foot under the last row, the corners turned — and no cell ruled on four sides",
      frame.headFirst.top === one && frame.headFirst.left === one && frame.headFirst.right === "0px" && frame.headFirst.bottom === "0px" &&
        frame.headMid.left === "0px" && frame.headMid.right === "0px" && frame.headLast.right === one && frame.midFirst.top === one &&
        frame.footMid.bottom === one && frame.headFirst.ink === frame.divider && frame.corner === "6px" && frame.footCorner === "6px",
      JSON.stringify(frame));
    ok("table: the head stands on a ground the rows do not, the frame hugs a narrow grid, and a wide table scrolls inside its own box — the page and the list do not move sideways",
      frame.headGround !== frame.bodyGround && frame.headGround !== "rgba(0, 0, 0, 0)" && frame.hugs && frame.broadScrolls && frame.broadInside &&
        frame.sideways <= 0 && frame.pageSideways <= 0,
      JSON.stringify(frame));
    ok("table: each table wears one copy beside its head row — a mark named twice, out of sight until the pointer or the keyboard reaches it, outside the grid so it covers no cell's words and is no part of a column's header, and in view beside a table that scrolls inside its box",
      frame.copies === 2 && frame.copyBeside && frame.headButtons === 0 && frame.copyName === "표 복사" && frame.copyTip === "표 복사" && frame.copyWords === "" &&
        frame.copyAtRest === "0" && frame.copyClear && frame.broadCopyInView && frame.broadRoom >= 28,
      JSON.stringify(frame));

    await page.locator(".pane-chat .md-table").first().hover();
    await page.waitForFunction(() => getComputedStyle(document.querySelector(".pane-chat .helper-table-copy")).opacity === "1");
    await page.locator(".pane-chat .helper-table-copy").first().click();
    await page.waitForSelector(".composer-menu");
    const menu = await page.evaluate(() => ({
      head: document.querySelector(".composer-menu .composer-menu-head")?.textContent ?? null,
      rows: [...document.querySelectorAll(".composer-menu .composer-menu-item .composer-menu-name")].map((row) => row.textContent),
      expanded: document.querySelector(".pane-chat .helper-table-copy").getAttribute("aria-expanded"),
    }));
    ok("table: under the pointer the copy shows, and a press opens the two ways to take the table away",
      menu.head === "표 복사" && menu.rows.join("|") === "마크다운으로 복사|CSV로 복사" && menu.expanded === "true", JSON.stringify(menu));

    await page.locator(".composer-menu .composer-menu-item", { hasText: "마크다운으로 복사" }).click();
    await page.waitForFunction(() => document.querySelector(".pane-chat .helper-table-copy use")?.getAttribute("href") === "#i-check");
    const asMarkdown = await page.evaluate(() => ({
      written: window.__CLIPBOARD_WRITES__.at(-1), menus: document.querySelectorAll(".composer-menu").length,
      focus: document.activeElement?.classList.contains("helper-table-copy") ?? false,
      name: document.querySelector(".pane-chat .helper-table-copy").getAttribute("aria-label"),
    }));
    ok("table: as markdown it is the table GFM writes — the head, the rule with each column's alignment, the cells with their code and bold marks — and the mark says it copied with its name kept",
      asMarkdown.written === [
        "| 대화 크기 | 지금 | 고친 뒤 |",
        "| :--- | ---: | :---: |",
        "| 400턴 | `210 ms` | **38 ms** |",
        "| 4,000턴, \"큰\" 대화 | 1,840 ms | 41 ms |",
      ].join("\n") && asMarkdown.menus === 0 && asMarkdown.focus && asMarkdown.name === "표 복사",
      JSON.stringify(asMarkdown));

    await page.locator(".pane-chat .helper-table-copy").first().focus();
    await page.keyboard.press("Enter");
    await page.waitForSelector(".composer-menu");
    await page.locator(".composer-menu .composer-menu-item", { hasText: "CSV로 복사" }).click();
    const asCsv = await page.evaluate(() => window.__CLIPBOARD_WRITES__.at(-1));
    ok("table: as CSV a field with a comma or a quote stands in quotes, its own quotes doubled — opened from the keyboard",
      asCsv === ["대화 크기,지금,고친 뒤", "400턴,210 ms,38 ms", "\"4,000턴, \"\"큰\"\" 대화\",\"1,840 ms\",41 ms"].join("\n"), JSON.stringify(asCsv));
    ok("table: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* A code block's bar says what the fence says of the code — the file it names, its language — and keeps the
 * list's top while a long block is read; a block past 24 lines stands folded behind a door that counts its
 * lines; 「줄바꿈」 joins the bar when a hand reaches the block, and is one reading for every block. */
export async function testConversationCode(browser, origin, ok) {
  const fence = "\x60\x60\x60";
  const long = Array.from({ length: 40 }, (_, at) => at === 3
    ? `const wide_${at} = "${"아주 긴 줄입니다 ".repeat(30)}";`
    : `const value_${at} = compute(${at});`).join("\n");
  const answer = [
    "고친 곳입니다.",
    "",
    `${fence}rust title="src/session.rs"`, "fn open() {", "    read_head();", "}", fence,
    "",
    "긴 파일도 하나 있습니다.",
    "",
    `${fence}ts:ui/list.ts`, long, fence,
    "",
    `${fence}sh`, "cargo test", fence,
    "",
    fence, "plain words", fence,
    "",
    "끝입니다.",
  ].join("\n");
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const parsed = await page.evaluate(() => [
      "rust", "rust title=\"src/a.rs\"", "ts:ui/list.ts", "12:15:src/a.rs", "src/session.rs", "C:\\src\\a.rs", "c++", "", "python3.11", "title=notes.md",
    ].map((info) => { const { language, file } = fenceInfoOf(info); return `${language}|${file}`; }));
    ok("code: a fence's first line is read for its language and the file it names — an attribute, 「lang:path」, a cited range, a path alone, a drive's colon — and a word with no slash and no extension is a language",
      parsed.join(" ; ") === "rust| ; rust|src/a.rs ; ts|ui/list.ts ; rs|src/a.rs ; rs|src/session.rs ; rs|C:\\src\\a.rs ; c++| ; | ; python3.11| ; |notes.md",
      JSON.stringify(parsed));

    await openSettled(page, [{ role: "user", text: "고친 곳을 보여 줘." }, { role: "assistant", text: answer }]);
    const read = () => page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const row = list.querySelector(":scope > .helper-turn.is-assistant");
      const frames = [...row.querySelectorAll(".helper-said .helper-code")];
      return {
        blocks: frames.map((frame) => {
          const bar = frame.querySelector(":scope > .helper-code-bar");
          const pre = frame.querySelector(":scope > pre");
          const door = frame.querySelector(":scope > .helper-code-more");
          const style = getComputedStyle(pre);
          return {
            file: bar.querySelector(".helper-code-file")?.textContent ?? null, language: bar.querySelector(".helper-code-lang").textContent,
            dataLanguage: pre.dataset.language ?? null, dataFile: pre.dataset.file ?? null,
            copy: bar.querySelector(".helper-code-copy").textContent, copyName: bar.querySelector(".helper-code-copy").getAttribute("aria-label"),
            wrap: bar.querySelector(".helper-code-wrap")?.getAttribute("aria-pressed") ?? null,
            long: frame.classList.contains("is-long"), open: frame.classList.contains("is-open"),
            door: door?.textContent ?? null, expanded: door?.getAttribute("aria-expanded") ?? null,
            height: Math.round(pre.getBoundingClientRect().height), maxHeight: style.maxHeight, overflowY: style.overflowY, whiteSpace: style.whiteSpace,
            top: Math.round(frame.getBoundingClientRect().top),
            barSticks: getComputedStyle(bar).position, barGround: getComputedStyle(bar).backgroundColor,
          };
        }),
        kept: row.__codeOpen === undefined ? null : row.__codeOpen === "all" ? "all" : [...row.__codeOpen],
        wrapRoot: document.documentElement.dataset.chatCodeWrap ?? null,
        sideways: list.scrollWidth - list.clientWidth,
      };
    });
    const start = await read();
    const [named, lengthy, shell, bare] = start.blocks;
    ok("code: the bar says the file the fence names and its language, a fence with a language says that, one with neither says 「코드」 — and the code keeps the language alone for its colours",
      named.file === "src/session.rs" && named.language === "rust" && named.dataLanguage === "rust" && named.dataFile === "src/session.rs" &&
        lengthy.file === "ui/list.ts" && lengthy.language === "ts" && shell.file === null && shell.language === "sh" &&
        bare.file === null && bare.language === "코드" && bare.dataLanguage === null &&
        start.blocks.every((block) => block.copy === "복사" && block.copyName === "코드 복사"),
      JSON.stringify(start.blocks.map(({ file, language, dataLanguage }) => ({ file, language, dataLanguage }))));
    ok("code: a block of 40 lines stands folded at 400px behind 「전체 보기 (40줄)」; the short ones have no door",
      lengthy.long && !lengthy.open && lengthy.door === "전체 보기 (40줄)" && lengthy.expanded === "false" && lengthy.maxHeight === "400px" &&
        lengthy.overflowY === "hidden" && lengthy.height <= 400 && !named.long && named.door === null && !shell.long && !bare.long,
      JSON.stringify(lengthy));
    // Opaque: a colour with no alpha part, however the engine spells it (`rgb(…)`, `color(srgb …)`).
    ok("code: in an answer the bar keeps the list's top, on an opaque ground", start.blocks.every((block) => block.barSticks === "sticky" &&
      /^(rgb|color)\(/.test(block.barGround) && !block.barGround.includes("/")), JSON.stringify(start.blocks.map((block) => [block.barSticks, block.barGround])));

    await page.locator(".pane-chat .helper-code-more").click();
    const opened = await read();
    ok("code: the door opens the whole block and says 「접기」, and the row remembers it",
      opened.blocks[1].open && opened.blocks[1].door === "접기" && opened.blocks[1].expanded === "true" && opened.blocks[1].height > 400 &&
        JSON.stringify(opened.kept) === "[1]",
      JSON.stringify({ block: opened.blocks[1], kept: opened.kept }));
    // Folded from below its head, the block's head comes back into view.
    const folded = await page.evaluate(async () => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const frame = list.querySelector(".helper-code.is-long");
      list.scrollTop += frame.getBoundingClientRect().bottom - list.getBoundingClientRect().top - 120;
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      const before = Math.round(frame.getBoundingClientRect().top - list.getBoundingClientRect().top);
      frame.querySelector(".helper-code-more").click();
      await window.__PAINTED__();
      return { before, after: Math.round(frame.getBoundingClientRect().top - list.getBoundingClientRect().top), open: frame.classList.contains("is-open") };
    });
    ok("code: folded from its foot, the block's head is back at the list's top rather than above it", folded.before < 0 && folded.after >= 0 && !folded.open, JSON.stringify(folded));

    // 「줄바꿈」 is built by the first reach — the page as it opened carried none, and the press on the long
    // block's door was a hand on that block — and is one reading for every block.
    const untouched = start.blocks.map((block) => block.wrap);
    await page.locator(".pane-chat .helper-code").nth(1).hover();
    await page.waitForSelector(".pane-chat .helper-code-wrap");
    const reached = await read();
    await page.locator(".pane-chat .helper-code-wrap").click();
    const wrapped = await read();
    const mark = await page.evaluate(() => getComputedStyle(document.querySelector(".pane-chat .helper-code-wrap"), "::before").content);
    await page.locator(".pane-chat .helper-code").nth(0).hover();
    await page.waitForFunction(() => document.querySelectorAll(".pane-chat .helper-code-wrap").length === 2);
    const second = await read();
    ok("code: no block carries 「줄바꿈」 until a hand reaches it; then that block's bar does, off",
      untouched.every((one) => one === null) && reached.blocks.map((block) => block.wrap).join() === ",false,," && reached.wrapRoot === null,
      JSON.stringify({ untouched, reached: reached.blocks.map((block) => block.wrap) }));
    ok("code: pressed, every block wraps its long lines and the pressed block has not moved; the control wears a check and says it is on, and a block reached later is born saying so",
      wrapped.wrapRoot === "on" && wrapped.blocks.every((block) => block.whiteSpace === "pre-wrap") && wrapped.blocks[1].wrap === "true" &&
        Math.abs(wrapped.blocks[1].top - reached.blocks[1].top) <= 1 && mark === '"✓ "' && second.blocks[0].wrap === "true" && wrapped.sideways <= 0,
      JSON.stringify({ root: wrapped.wrapRoot, tops: [reached.blocks[1].top, wrapped.blocks[1].top], mark, second: second.blocks[0].wrap }));
    await page.locator(".pane-chat .helper-code-wrap").first().click();
    const unwrapped = await read();
    ok("code: pressed again, the lines are their own again", unwrapped.wrapRoot === null && unwrapped.blocks.every((block) => block.whiteSpace === "pre" && block.wrap !== "true"),
      JSON.stringify(unwrapped.blocks.map((block) => [block.whiteSpace, block.wrap])));
    ok("code: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
  await testConversationCodeStream(browser, origin, ok, answer);
}

/* An answer read as it came keeps its long code open when its turn takes the streaming row's place; a page
 * that opens the same turn later folds it. */
async function testConversationCodeStream(browser, origin, ok, answer) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openConversation(page, [{ role: "user", text: "고친 곳을 보여 줘." }], { status: "working", live: [{ role: "assistant", text: `${answer}\n\n아직 쓰는 중` }] });
    await foldConversations(page);
    await page.evaluate(async () => { for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__(); });
    const streaming = await page.evaluate(() => {
      const row = document.querySelector("#worker-view .helper-turns > .is-streaming.is-assistant");
      const pre = [...row.querySelectorAll(".helper-code > pre")].find((one) => one.textContent.includes("value_39"));
      return { blocks: row.querySelectorAll(".helper-code").length, doors: row.querySelectorAll(".helper-code-more").length,
        height: Math.round(pre?.getBoundingClientRect().height ?? 0) };
    });
    ok("stream: an answer still being written folds none of its code — it is read as it comes",
      streaming.blocks === 4 && streaming.doors === 0 && streaming.height > 400, JSON.stringify(streaming));
    const settled = await page.evaluate(async (answer) => {
      window.__CONVERSATION__.turns.push({ role: "assistant", text: `${answer}\n\n아직 쓰는 중` });
      window.__CONVERSATION__.live = [];
      window.__CONVERSATION__.status = "idle";
      await pollHelperPages();
      for (let beat = 0; beat < 3; beat += 1) await window.__PAINTED__();
      const list = document.querySelector("#worker-view .helper-turns");
      const row = list.querySelector(":scope > .helper-turn.is-assistant:not(.is-streaming)");
      const frame = row?.querySelector(".helper-code.is-long");
      const seen = {
        streaming: list.querySelectorAll(":scope > .is-streaming").length, long: Boolean(frame), open: frame?.classList.contains("is-open") ?? null,
        door: frame?.querySelector(".helper-code-more")?.textContent ?? null, kept: row?.__codeOpen ?? null,
        height: Math.round(frame?.querySelector("pre").getBoundingClientRect().height ?? 0),
      };
      // The same turn on a row built anew — a page that opens it later — stands folded.
      const again = helperTurnRowNode(list.__run, row.__turn, -1);
      list.appendChild(again);
      seen.later = again.querySelector(".helper-code.is-long")?.classList.contains("is-open") ?? null;
      again.remove();
      return seen;
    }, answer);
    ok("stream: its turn takes the row's place with the long block still open — 「접기」 under it — and the same turn drawn later stands folded",
      settled.streaming === 0 && settled.long && settled.open === true && settled.door === "접기" && settled.kept === "all" &&
        settled.height > 400 && settled.later === false,
      JSON.stringify(settled));
    ok("stream: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* What a command printed, opened: an output long by its lines stands as its first three lines and its last
 * three around one line that says how many it holds back; the copy takes the whole; find walks into a line
 * it held back without taking the keyboard. */
export async function testConversationLongOutput(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    const printed = ["running 214 tests", ...Array.from({ length: 214 }, (_, at) => `test case_${String(at).padStart(3, "0")} ... ok`),
      "", "test result: ok. 214 passed; 0 failed"].join("\n");
    await openSettled(page, [
      { role: "user", text: "테스트를 돌려 줘." },
      { role: "tool", text: "Bash cargo test -p session", tool: { call_id: "t1", name: "Bash", kind: "bash", input: "cargo test -p session" } },
      { role: "tool_result", text: printed, tool: { call_id: "t1", is_error: false } },
      { role: "tool", text: "Bash git status --short", tool: { call_id: "t2", name: "Bash", kind: "bash", input: "git status --short" } },
      { role: "tool_result", text: " M src/a.rs\n M src/b.rs\n M src/c.rs\n M src/d.rs\n?? notes.txt", tool: { call_id: "t2", is_error: false } },
      { role: "assistant", text: "모두 통과했습니다." },
    ]);
    const open = await page.evaluate(async () => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const fold = list.querySelector(":scope > .helper-group");
      fold.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      const rows = [...fold.querySelectorAll(":scope > .helper-group-body > .is-tool")];
      for (const row of rows) row.querySelector("summary").click();
      await window.__PAINTED__();
      await window.__PAINTED__();
      window.__OUT_ROWS__ = rows;
      return rows.length;
    });
    const read = () => page.evaluate(() => window.__OUT_ROWS__.map((row) => {
      const words = row.querySelector(".helper-tool-output");
      const gap = words.querySelector(".helper-tool-gap");
      const door = words.nextElementSibling?.classList.contains("helper-expand") ? words.nextElementSibling : null;
      return {
        ends: words.classList.contains("is-ends"), clipped: words.classList.contains("is-clipped"), open: words.classList.contains("is-open"),
        lines: words.querySelector(".helper-tool-text").textContent.split("\n").length,
        text: words.querySelector(".helper-tool-text").textContent.slice(0, 60),
        tail: words.querySelector(".helper-tool-text").textContent.slice(-40),
        gap: gap?.textContent ?? null, gapIsButton: gap?.tagName === "BUTTON", gapExpanded: gap?.getAttribute("aria-expanded") ?? null,
        door: door?.textContent ?? null, focusOnDoor: door !== null && document.activeElement === door, focusOnGap: gap !== null && document.activeElement === gap,
        resLine: row.querySelector(".helper-step-res").textContent,
      };
    }));
    const [long, short] = await read();
    ok("output: 217 lines stand as the first three and the last three around one line that says how many it holds back — the line that says how the run went is on screen",
      open === 2 && long.ends && !long.clipped && !long.open && long.gap === "… 211줄 더" && long.gapIsButton && long.gapExpanded === "false" &&
        long.text.startsWith("running 214 tests") && long.tail.endsWith("test result: ok. 214 passed; 0 failed") && long.door === null,
      JSON.stringify(long));
    ok("output: five lines keep the clip's grammar — no ends, no line between", !short.ends && short.gap === null && short.clipped, JSON.stringify(short));

    const copied = await page.evaluate(async () => {
      const row = window.__OUT_ROWS__[0];
      const copy = row.querySelector(".helper-tool-output").parentElement.querySelector(".helper-code-copy");
      copy.click();
      await window.__PAINTED__();
      const written = window.__CLIPBOARD_WRITES__.at(-1);
      return { lines: written.split("\n").length, last: written.split("\n").at(-1), holdsMiddle: written.includes("case_100") && !written.includes("줄 더") };
    });
    ok("output: its copy takes the whole of it, not the two ends", copied.lines === 217 && copied.last === "test result: ok. 214 passed; 0 failed" && copied.holdsMiddle,
      JSON.stringify(copied));

    await page.locator(".pane-chat .helper-tool-gap").focus();
    await page.keyboard.press("Enter");
    const [whole] = await read();
    ok("output: Enter on the line opens the rest in place and the keyboard stands on 「접기」 under it",
      whole.open && whole.gap === null && whole.lines === 217 && whole.door === "접기" && whole.focusOnDoor, JSON.stringify(whole));
    await page.keyboard.press("Enter");
    const [again] = await read();
    ok("output: 「접기」 stands the two ends again, the keyboard on the line between them",
      !again.open && again.gap === "… 211줄 더" && again.door === null && again.focusOnGap, JSON.stringify(again));

    // Find walks into a line the ends held back, and the keyboard stays in the find's box.
    await page.locator(".pane-chat .chat-head-find").click();
    await page.locator(".pane-chat .chat-find input").fill("case_100");
    await page.waitForFunction(() => window.__OUT_ROWS__[0].querySelector(".helper-tool-output").classList.contains("is-open"));
    const found = await page.evaluate(() => {
      const words = window.__OUT_ROWS__[0].querySelector(".helper-tool-output");
      return { open: words.classList.contains("is-open"), holds: words.textContent.includes("case_100"), inFind: document.activeElement?.closest(".chat-find") !== null };
    });
    ok("output: a find for a line the ends held back stands the whole output, with the keyboard still in the find's box",
      found.open && found.holds && found.inFind, JSON.stringify(found));
    ok("output: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}

/* The words a conversation was opened with — sent from the start page — stand at the top of its page with
 * the answer under them, as words sent from the page's own composer do; a conversation opened any other way
 * stands at its foot as before. */
export async function testConversationFirstAsk(browser, origin, ok) {
  const answer = Array.from({ length: 60 }, (_, line) => `첫 답의 ${line + 1}번째 문단입니다. 화면 하나를 넘길 만큼 깁니다.`).join("\n\n");
  const open = async (first) => {
    const { page, faults } = await openWindowTestPage(browser, origin);
    try {
      await standZo(page);
      const seen = await page.evaluate(async ({ first, answer }) => {
        const turns = [{ role: "user", text: "로그인 화면의 오류를 고쳐 줘." }, { role: "assistant", text: answer }];
        const term = await openTermTab({ placement: "tab" });
        paneAgents.set(term, "zo");
        hookStates.set(term, "working");
        window.__ANSWER__.pane_log = (args) => ({
          found: true, next: turns.length, turns: args.after == null ? turns : turns.slice(args.after),
          skipped: false, more: false, folded: false, model: "claude-opus-5",
        });
        // What the start page does once the agent's pane stands (`sendNewChat`).
        if (first) noteFirstAsk(term);
        await setPaneChat(term, true);
        await pollHelperPages();
        for (let beat = 0; beat < 4; beat += 1) await window.__PAINTED__();
        const list = document.querySelector(".pane-chat .helper-turns");
        const asked = list.querySelector(":scope > .helper-turn.is-user");
        const state = list.__follow;
        return {
          under: Math.round(asked.getBoundingClientRect().top - list.getBoundingClientRect().top),
          gap: Math.round(list.scrollHeight - list.scrollTop - list.clientHeight - (state?.room ?? 0)),
          pinned: Boolean(state?.pin), wish: state?.pinNext ?? null, door: list.__door?.classList.contains("is-shown") ?? null,
          left: chatFirstAsks.size,
        };
      }, { first, answer });
      return { seen, faults };
    } finally {
      await page.close();
    }
  };
  const asked = await open(true);
  const plain = await open(false);
  ok("first: the words a conversation was opened with stand at the top of its page, the long answer under them and the button back to the foot beside it",
    asked.seen.pinned && Math.abs(asked.seen.under - 12) <= 2 && asked.seen.gap > 50 && asked.seen.door === true && asked.seen.wish === false && asked.seen.left === 0,
    JSON.stringify(asked.seen));
  ok("first: a conversation opened any other way stands at its foot as before", !plain.seen.pinned && plain.seen.gap <= 1 && plain.seen.under < 0,
    JSON.stringify(plain.seen));
  ok("first: no page errors", asked.faults.length === 0 && plain.faults.length === 0, [...asked.faults, ...plain.faults].join("\n"));
}

/* Between two turns: 32px from an answer's words to the next person's, 12px inside a turn — and an older
 * answer's marks, when a hand brings them, stand in that gap, take their presses there and move nothing. */
export async function testConversationTurnGap(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    // A window wide enough that a person's words keep their foot line out of the flow (the page's 720px step).
    await page.setViewportSize({ width: 1440, height: 900 });
    await openAfterScene(page);
    await foldConversations(page);
    await page.evaluate(() => window.__PAINTED__());
    const gaps = await page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const rows = [...list.querySelectorAll(":scope > [data-turn]")];
      const wordsOf = (row) => row.querySelector(":scope > .helper-said, :scope > .helper-group-cap").getBoundingClientRect();
      const steps = [];
      for (let at = 1; at < rows.length; at += 1) {
        steps.push({
          from: rows[at - 1].classList.contains("is-user") ? "person" : rows[at - 1].classList.contains("helper-group") ? "fold" : "answer",
          to: rows[at].classList.contains("is-user") ? "person" : rows[at].classList.contains("helper-group") ? "fold" : "answer",
          gap: Math.round(wordsOf(rows[at]).top - wordsOf(rows[at - 1]).bottom),
        });
      }
      return steps;
    });
    const between = gaps.filter((step) => step.from === "answer" && step.to === "person");
    const inside = gaps.filter((step) => !(step.from === "answer" && step.to === "person"));
    ok("gap: 32px from a turn's answer to the next turn's words, 12px between the words, the work's line and the answer inside a turn",
      between.length === 1 && between.every((step) => Math.abs(step.gap - 32) <= 1) && inside.length >= 3 && inside.every((step) => Math.abs(step.gap - 12) <= 1),
      JSON.stringify(gaps));

    // The older answer's marks, brought by the pointer: in the gap, over nothing, each taking its press.
    const older = page.locator(".pane-chat .helper-turns > .helper-turn.is-assistant.is-turn-answer").first();
    const before = await page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      return { height: list.scrollHeight, top: Math.round(list.scrollTop) };
    });
    await older.hover();
    await page.waitForFunction(() => {
      const row = document.querySelector(".pane-chat .helper-turns > .helper-turn.is-assistant.is-turn-answer");
      const actions = row.querySelector(":scope > .helper-actions");
      return actions !== null && getComputedStyle(actions).opacity === "1";
    });
    const marks = await page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const row = list.querySelector(":scope > .helper-turn.is-assistant.is-turn-answer");
      const next = [...list.querySelectorAll(":scope > .helper-turn.is-user")].find((one) => one.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_PRECEDING);
      const words = row.querySelector(":scope > .helper-said").getBoundingClientRect();
      const bubble = next.querySelector(":scope > .helper-said").getBoundingClientRect();
      const shown = [...row.querySelectorAll(":scope > .helper-actions > *")].filter((mark) => !mark.hidden && mark.getClientRects().length > 0);
      return {
        count: shown.length,
        inGap: shown.every((mark) => { const box = mark.getBoundingClientRect(); return box.top >= words.bottom - 1 && box.bottom <= bubble.top + 1; }),
        // Each mark at three heights — its top, its middle and its foot, the part that stands over the next row's pad.
        pressed: shown.every((mark) => {
          const box = mark.getBoundingClientRect();
          return [box.top + 2, box.top + box.height / 2, box.bottom - 2].every((y) => {
            const hit = document.elementFromPoint(box.left + box.width / 2, y);
            return hit === mark || mark.contains(hit);
          });
        }),
        height: list.scrollHeight, top: Math.round(list.scrollTop),
        newestKeeps: (() => {
          const latest = list.querySelector(":scope > .helper-turn.is-latest");
          const actions = latest.querySelector(":scope > .helper-actions");
          return actions === null || actions.getBoundingClientRect().bottom <= latest.getBoundingClientRect().bottom + 1;
        })(),
      };
    });
    ok("gap: an older answer's marks stand in the gap under it — between its words and the next person's — every one of them takes its press along its whole height, and bringing them moved nothing",
      marks.count >= 3 && marks.inGap && marks.pressed && marks.height === before.height && marks.top === before.top, JSON.stringify({ before, marks }));
    ok("gap: the newest answer keeps its marks' room in the page's own flow", marks.newestKeeps, JSON.stringify(marks));
    ok("gap: no page errors", faults.length === 0, faults.join("\n"));
  } finally {
    await page.close();
  }
}
