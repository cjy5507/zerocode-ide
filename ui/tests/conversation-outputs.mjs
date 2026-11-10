/* What an answer is made of, part by part — the third stage of the conversation
 * the person passed on 2026-11-10 (window artifact p-1c1bbf75726ff227, 「출력
 * 형태」): words nobody typed are a thin line, never the person's bubble. Each
 * check reads the page a browser laid out. The conversations are synthetic.
 *
 *   WINDOW_SUITES='^conversation-outputs' node ui/tests/window.mjs */
import { openPaneConversation, standZo } from "./conversation-parity.mjs";
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
        const why = fold.querySelector(":scope > .helper-group-why");
        const style = why ? getComputedStyle(why) : null;
        return {
          failed: fold.classList.contains("is-failed"), words: fold.querySelector(".helper-group-words").textContent,
          mark: getComputedStyle(fold.querySelector(".helper-group-cap"), "::before").content,
          why: why?.textContent ?? null, shown: why ? why.getClientRects().length > 0 : false,
          oneLine: style ? style.whiteSpace === "nowrap" && style.textOverflow === "ellipsis" : null,
          inside: why ? why.getBoundingClientRect().right <= list.getBoundingClientRect().right + 1 : null,
          afterCap: why?.previousElementSibling?.classList.contains("helper-group-cap") ?? null,
        };
      });
    });
    const [broke, whole] = await read();
    ok("failed: a fold with a failure says which step failed and the first line it printed — past a bare exit code — under its own line, on one line inside the column",
      broke.failed && broke.words.includes("실패 2") && broke.mark === '"✕"' && broke.shown && broke.afterCap && broke.oneLine && broke.inside &&
        broke.why === "명령 cargo test -p session — error[E0425]: cannot find function `read_head` in this scope",
      JSON.stringify(broke));
    ok("failed: a fold whose work came back whole has no such line", !whole.failed && whole.why === null && whole.mark === '"✓"', JSON.stringify(whole));

    const opened = await page.evaluate(async () => {
      const fold = document.querySelector(".pane-chat .helper-turns > .helper-group");
      fold.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      const rows = [...fold.querySelectorAll(":scope > .helper-group-body > .is-tool")];
      return {
        whyShown: fold.querySelector(":scope > .helper-group-why").getClientRects().length > 0,
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
      return fold.querySelector(":scope > .helper-group-why").getClientRects().length > 0;
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
      const copy = table.querySelector(".helper-table-copy");
      const prose = table.parentElement.querySelector(".md-para");
      return {
        divider: probe("--chat-divider"),
        headFirst: edge(rows[0].cells[0]), headMid: edge(rows[0].cells[1]), headLast: edge(rows[0].cells[2]),
        midFirst: edge(rows[1].cells[0]), footMid: edge(rows[2].cells[1]),
        corner: getComputedStyle(rows[0].cells[0]).borderTopLeftRadius, footCorner: getComputedStyle(rows[2].cells[2]).borderBottomRightRadius,
        headGround: getComputedStyle(rows[0].cells[0]).backgroundColor, bodyGround: getComputedStyle(rows[1].cells[0]).backgroundColor,
        hugs: Math.round(box(rows[0].cells[2]).right) < Math.round(box(prose).right) - 40,
        broadScrolls: broad.scrollWidth > broad.clientWidth + 1, broadInside: Math.round(box(broad).right) <= Math.round(box(list).right),
        sideways: list.scrollWidth - list.clientWidth, pageSideways: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        copies: list.querySelectorAll(".helper-table-copy").length, copyInHead: copy?.parentElement === rows[0].cells[2],
        copyName: copy?.getAttribute("aria-label"), copyTip: copy?.dataset.tip, copyWords: copy?.textContent, copyAtRest: copy ? getComputedStyle(copy).opacity : null,
        copyInsideCell: copy ? box(copy).right <= box(rows[0].cells[2]).right && box(copy).top >= box(rows[0].cells[2]).top - 1 : null,
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
    ok("table: each table wears one copy at the end of its head row — a mark named twice, out of sight until the pointer or the keyboard reaches it",
      frame.copies === 2 && frame.copyInHead && frame.copyName === "표 복사" && frame.copyTip === "표 복사" && frame.copyWords === "" &&
        frame.copyAtRest === "0" && frame.copyInsideCell,
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
