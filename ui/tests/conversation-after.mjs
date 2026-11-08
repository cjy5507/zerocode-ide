import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium, createWindowServer, foldConversations, openWindowTestPage } from "./window-boot.mjs";
import { openPaneConversation, standZo } from "./conversation-parity.mjs";

export const afterTurns = [
  { role: "user", text: "문서의 제목을 정리해줘." },
  { role: "tool", text: "Edit docs.md", tool: { call_id: "old", name: "Edit", kind: "edit", input: "docs.md", file: { path: "docs.md" }, edits: [{ path: "docs.md", lines: [{ kind: "add", text: "# Earlier change" }] }] } },
  { role: "tool_result", text: "updated", tool: { call_id: "old", is_error: false } },
  { role: "assistant", text: "문서 제목을 정리했습니다." },
  { role: "user", text: "큰 대화를 다시 열 때 멈추는 원인을 찾아서 수정해줘." },
  { role: "assistant", text: "먼저 세션을 읽는 경로와 화면 갱신을 확인하겠습니다." },
  { role: "tool", text: "Read src/session.rs", tool: { call_id: "read", name: "Read", kind: "read", input: "src/session.rs", file: { path: "src/session.rs" } } },
  { role: "tool_result", text: "open_session → replay_history", tool: { call_id: "read", is_error: false } },
  { role: "tool", text: "Edit src/session.rs", tool: { call_id: "edit", name: "Edit", kind: "edit", input: "src/session.rs", file: { path: "src/session.rs" }, edits: [{ path: "src/session.rs", lines: [
    { kind: "del", text: "let session = open_session(id)?;" },
    { kind: "add", text: "let session = load_session(id).await?;" },
    { kind: "add", text: "show_session(session);" },
  ] }] } },
  { role: "tool_result", text: "updated", tool: { call_id: "edit", is_error: false } },
  { role: "tool", text: "Bash cargo test", tool: { call_id: "test", name: "Bash", kind: "bash", input: "cargo test" } },
  { role: "tool_result", text: "running 3 tests\nresume_can_be_cancelled ... ok\nfirst_byte_after_restore ... ok\ntest result: 3 passed; 0 failed", tool: { call_id: "test", is_error: false } },
  { role: "assistant", text: "세션을 여는 동안 화면 갱신이 기다리는 경로를 찾았습니다. 로딩을 별도 작업으로 옮기고, 취소하면 기존 대화를 유지하도록 바꿨습니다." },
].map((turn, message_index) => ({ ...turn, message_index, session_id: "after-session" }));

export async function openAfterScene(page) {
  await standZo(page);
  await openPaneConversation(page, afterTurns);
  await page.evaluate(async () => {
    const term = activePaneOf(currentTab());
    paneSessions.set(term, { agent: "zo", session: { id: "after-session" } });
    panePrompts.set(term, "세션 복구 개선");
    hookStates.set(term, "done");
    await pollHelperPages();
    paintPaneChat(term);
    await window.__PAINTED__();
  });
}

export async function testConversationAfter(browser, origin, ok, screenshots = null) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openAfterScene(page);
    ok("The header names the actual conversation beside its branch", await page.locator(".pane-chat .chat-heading .worker-name").textContent() === "세션 복구 개선");
    const review = page.locator(".pane-chat .chat-answer-review");
    await review.click();
    const panel = page.locator(".pane-chat .chat-review");
    const text = await panel.textContent();
    ok("The answer reviews its own turn, not an earlier turn's edits", text.includes("src/session.rs") && !text.includes("docs.md"), text);
    ok("The action carries the recorded change tally", (await review.textContent()).includes("+2 −1"));
    await page.keyboard.press("Escape");
    ok("Escape closes the review and returns keyboard focus to its action", await panel.count() === 0 && await review.evaluate((node) => node === document.activeElement));
    await page.locator(".pane-chat .chat-head-find").click();
    await page.locator(".pane-chat .chat-find input").fill("first_byte");
    const found = await page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const selected = CSS.highlights.get("chat-find-active-match");
      const range = selected ? [...selected][0] : null;
      const words = range?.startContainer.parentElement?.closest(".helper-tool-output");
      return { match: range?.toString(), output: words?.textContent,
        open: words?.closest("details")?.open,
        count: document.querySelector(".pane-chat .file-find-count")?.textContent };
    });
    ok("Find opens and highlights a match inside folded tool output", found.match === "first_byte" && found.open && found.count === "1/1", JSON.stringify(found));
    const marker = await page.evaluate(() => {
      const range = [...(CSS.highlights.get("chat-find-active-match") ?? [])][0];
      const words = range?.startContainer.parentElement?.closest(".helper-tool-output");
      const markers = [...document.querySelectorAll(".pane-chat .chat-find-marker")];
      return { count: markers.length, text: markers[0]?.textContent, beside: Boolean(words) && markers[0]?.nextElementSibling?.contains(words) === true };
    });
    ok("A find that lands in a tool's raw output says so in one line at the target", marker.count === 1 && marker.text === "도구 원문의 검색 결과로 이동 · 같은 세션" && marker.beside, JSON.stringify(marker));
    await page.keyboard.press("Escape");
    ok("The jump marker leaves when the find bar closes", await page.locator(".pane-chat .chat-find-marker").count() === 0);
    const skin = await page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const person = list.querySelector(".is-user .helper-said");
      const action = list.querySelector(".chat-answer-review");
      const row = person.parentElement;
      return { right: row.getBoundingClientRect().right - person.getBoundingClientRect().right,
        radius: getComputedStyle(person).borderRadius, actionSize: getComputedStyle(action).fontSize,
        actionRadius: getComputedStyle(action).borderRadius };
    });
    ok("The approved prompt axis — the right, a 12px bubble (2026-11-08, decision 1) — and compact actions are drawn", Math.abs(skin.right) < 2 && skin.radius === "12px" && skin.actionSize === "11px" && skin.actionRadius === "6px", JSON.stringify(skin));
    for (const theme of ["dark", "light"]) {
      await page.evaluate((theme) => { document.documentElement.dataset.theme = theme; }, theme);
      for (const width of [1280, 360]) {
        await page.setViewportSize({ width, height: 860 });
        await page.evaluate(async () => { await window.__PAINTED__(); await window.__PAINTED__(); });
        const fits = await page.evaluate(() => {
          const list = document.querySelector(".pane-chat .helper-turns");
          const box = list.getBoundingClientRect();
          return [...list.querySelectorAll(".helper-actions button:not([hidden])")].every((button) => {
            const r = button.getBoundingClientRect(); return r.left >= box.left - 1 && r.right <= box.right + 1;
          });
        });
        ok(`Answer actions remain inside the conversation at ${width}px in ${theme}`, fits);
        if (screenshots) await page.screenshot({ path: resolve(screenshots, `after-${theme}-${width}.png`) });
      }
    }
    const failedEdit = await page.evaluate(() => {
      const host = document.querySelector(".pane-chat");
      const list = host.querySelector(".helper-turns");
      const run = list.__run;
      run.helper.turns.find((turn) => turn.tool?.call_id === "edit").outputError = true;
      updateConversationActions(list.__actions, list, list.__actions.__row);
      updateChatHead(host.querySelector(".chat-head"), run);
      return list.querySelector(".chat-answer-review").hidden && host.querySelector(".chat-head-review").hidden;
    });
    ok("A failed edit is excluded from both turn review and the header change count", failedEdit);
    ok("The after-view scene raises no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

export async function testConversationArchive(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openAfterScene(page);
    await page.evaluate(() => {
      const host = document.querySelector(".pane-chat");
      const run = host.__helperPage.run;
      run.helper.turns.forEach((turn, at) => { turn.line = 2000 + at * 40; });
      run.helper.next = 3000;
      run.helper.from = 2000;
      window.__ARCHIVE_READS__ = [];
      window.__ANSWER__.pane_log = (args) => {
        window.__ARCHIVE_READS__.push(args);
        if (args.archive) return { found: false, next: 0, start: 0, turns: [], more: false };
        if (!args.whole) return { found: true, next: 3000, turns: [], more: false };
        const older = args.before === 2000;
        return { found: true, skipped: false, start: older ? 0 : 2000, next: args.before,
          turns: older ? [
            { line: 800, role: "user", text: "Earlier task" },
            { line: 900, role: "tool", text: "Bash cargo test", tool: { call_id: "archived", name: "Bash", kind: "bash", input: "cargo test" } },
            { line: 1000, role: "tool_result", text: "archived_needle in original output", tool: { call_id: "archived", is_error: false } },
            { line: 1100, role: "assistant", text: "Earlier answer" },
          ] : [{ line: 2500, role: "assistant", text: "Current answer" }] };
      };
      const box = host.querySelector(".worker-composer-box");
      box.value = "보내지 않은 초안";
      box.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await page.locator(".pane-chat .chat-head-find").click();
    await page.locator(".pane-chat .chat-find input").fill("archived_needle");
    await page.waitForFunction(() => document.querySelector(".pane-chat .file-find-count")?.textContent === "1/1");
    const result = await page.evaluate(() => {
      const page = document.querySelector(".pane-chat").__helperPage;
      const ranges = CSS.highlights.get("chat-find-active-match");
      return { reads: window.__ARCHIVE_READS__, match: ranges ? [...ranges][0]?.toString() : null,
        draft: document.querySelector(".pane-chat .worker-composer-box").value,
        from: page.run.helper.from, cut: page.run.helper.cut,
        scope: page.find.scope.textContent };
    });
    ok("Full-history search crosses unloaded pages and reloads the matched original", result.match === "archived_needle" && result.from === 0 && result.cut && result.reads.filter((read) => read.whole && !read.archive).map((read) => read.before).join() === "3000,2000,2000", JSON.stringify(result));
    ok("Searching an archived tool log preserves the unsent composer draft", result.draft === "보내지 않은 초안");
    const archived = await page.evaluate(() => [...document.querySelectorAll(".pane-chat .chat-find-marker")].map((one) => one.textContent));
    ok("A find that jumps outside the loaded turns says so in one line at the target", archived.length === 1 && archived[0] === "이전 기록의 검색 결과로 이동 · 같은 세션", JSON.stringify(archived));
    await page.keyboard.press("Escape");
    ok("The earlier-history marker leaves when the find bar closes", await page.locator(".pane-chat .chat-find-marker").count() === 0);
    ok("Archive search raises no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

export async function testConversationRecovery(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openAfterScene(page);
    await page.evaluate(() => {
      window.__CONTROLS__ = [];
      window.__ANSWER__.pane_conversation_action = (args) => {
        window.__CONTROLS__.push(args);
        if (args.action.kind === "preview") return {
          session: "after-session", message_index: args.action.message_index, revision: "revision-1",
          prompt: "큰 대화를 다시 열 때 멈추는 원인을 찾아서 수정해줘.",
          conversation_available: true, files_available: true,
          files: { paths: ["src/session.rs"], conflicted: [], skipped: [], incomplete_range: false },
        };
        return new Promise((resolve) => { window.__CONTROL_DONE__ = resolve; });
      };
    });
    const rewind = page.locator('.pane-chat [data-conversation-control="restore"]');
    await rewind.click();
    const dialog = page.getByRole("dialog", { name: "이 턴 이전으로 돌아가기" });
    await dialog.waitFor();
    ok("Recovery presents the three approved scopes and the real target preview", await dialog.locator('input[type="radio"]').count() === 3 && (await dialog.textContent()).includes("src/session.rs"));
    await page.keyboard.press("Escape");
    ok("Closing a preview never restores or interrupts work", await page.evaluate(() => window.__CONTROLS__.length === 1 && window.__CONTROLS__[0].action.kind === "preview"));
    await rewind.click();
    await dialog.locator('input[value="conversation"]').check();
    const apply = dialog.getByRole("button", { name: "선택한 상태로 복원" });
    await page.keyboard.press("Shift+Tab");
    ok("Keyboard focus stays inside recovery after choosing another radio scope", await apply.evaluate((node) => node === document.activeElement));
    await apply.click();
    ok("An outstanding recovery cannot be submitted twice", await apply.isDisabled());
    const request = await page.evaluate(() => window.__CONTROLS__.at(-1));
    ok("Recovery uses the saved message index, scope, session and accepted revision", request.term !== undefined && request.session === "after-session" && request.action.kind === "restore" && request.action.message_index === 4 && request.action.scope === "conversation" && request.action.revision === "revision-1", JSON.stringify(request));
    await page.evaluate(() => window.__CONTROL_DONE__({ kind: "restored", scope: "conversation", prompt: "원래 요청" }));
    await dialog.waitFor({ state: "detached" });
    ok("Confirmed recovery seeds an empty composer with the selected original prompt", await page.locator(".pane-chat .worker-composer-box").inputValue() === "원래 요청");
    await page.locator('.pane-chat [data-conversation-control="fork"]').click();
    const fork = page.getByRole("dialog", { name: "이 대화에서 다른 접근 시작" });
    await fork.getByRole("button", { name: "분기해서 계속하기" }).click();
    const forked = await page.evaluate(() => window.__CONTROLS__.at(-1));
    ok("Fork is a separate revision-bound action, never a restore or a model prompt", forked.action.kind === "fork" && forked.action.revision === "revision-1" && !("scope" in forked.action));
    await page.evaluate(() => window.__CONTROL_DONE__({ kind: "forked", path: "new-session.jsonl" }));
    await fork.waitFor({ state: "detached" });
    ok("Recovery controls raise no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

/* The review panel's test line: read from the answer's own steps, a count only when the output carries one. */
export async function testConversationReviewTests(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openAfterScene(page);
    const line = (command, output, failed = false) => page.evaluate(async ({ command, output, failed }) => {
      const host = document.querySelector(".pane-chat");
      const list = host.querySelector(".helper-turns");
      closeConversationReview(host.__helperPage, false);
      const step = list.__run.helper.turns.find((turn) => turn.tool?.call_id === "test");
      step.tool.input = command;
      step.text = `Bash ${command}`;
      step.output = output;
      step.outputError = failed;
      list.querySelector(".chat-answer-review").click();
      await window.__PAINTED__();
      const said = [...host.querySelectorAll(".chat-review .chat-review-tests")];
      return said.length === 0 ? null : said.length === 1 ? said[0].textContent : `${said.length} lines`;
    }, { command, output, failed });
    const cases = [
      ["cargo's own count", "cargo test", "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored", false, "✓ 테스트 3개 통과"],
      ["cargo's count over several binaries", "cargo nextest run", "test result: ok. 3 passed; 0 failed\ntest result: FAILED. 4 passed; 1 failed", true, "✗ 테스트 8개 중 1개 실패"],
      ["node --test's count", "node --test ui/tests/a.test.mjs", "ℹ tests 6\nℹ pass 5\nℹ fail 1", true, "✗ 테스트 6개 중 1개 실패"],
      ["pytest's count", "python -m pytest -q", "..F\n=== 1 failed, 8 passed in 0.12s ===", true, "✗ 테스트 9개 중 1개 실패"],
      ["jest's count", "npx jest", "Tests:       4 passed, 4 total", false, "✓ 테스트 4개 통과"],
      ["vitest's count", "pnpm test", " Test Files  1 passed (1)\n      Tests  7 passed (7)", false, "✓ 테스트 7개 통과"],
      ["no count, success", "just window-runner-test", "all green", false, "✓ 테스트 성공 · just window-runner-test"],
      ["no count, failure", "go test ./...", "build failed", true, "✗ 테스트 실패 · go test ./..."],
    ];
    for (const [name, command, output, failed, expected] of cases) {
      const said = await line(command, output, failed);
      ok(`The review's test line reads ${name}`, said === expected, String(said));
    }
    ok("A turn without a test command has no test line", await line("ls -la", "total 0") === null);
    ok("A test step whose result never came says nothing", await line("cargo test", undefined) === null);
    ok("Review test lines raise no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

/* 「이 변경 되돌리기」 in the review head opens the one recovery dialog, files preselected — or says why not. */
export async function testConversationReviewUndo(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openAfterScene(page);
    await page.evaluate(() => {
      window.__CONTROLS__ = [];
      window.__FILES__ = { files_available: true, file_problem: null };
      window.__ANSWER__.pane_conversation_action = (args) => {
        window.__CONTROLS__.push(args);
        return { session: "after-session", message_index: args.action.message_index, revision: "revision-1",
          prompt: "큰 대화를 다시 열 때 멈추는 원인을 찾아서 수정해줘.", conversation_available: true,
          ...window.__FILES__,
          files: window.__FILES__.files_available ? { paths: ["src/session.rs"], conflicted: [], skipped: [], incomplete_range: false } : null };
      };
    });
    await page.locator(".pane-chat .chat-answer-review").click();
    const undo = page.locator(".pane-chat .chat-review .chat-review-head .chat-review-undo");
    await undo.waitFor();
    ok("The review head carries 「이 변경 되돌리기」", (await undo.textContent()) === "이 변경 되돌리기");
    await page.waitForFunction(() => document.querySelector(".pane-chat .chat-review-undo")?.disabled === false);
    await undo.click();
    const dialog = page.getByRole("dialog", { name: "이 턴 이전으로 돌아가기" });
    await dialog.waitFor();
    const chosen = await dialog.locator('input[type="radio"]:checked').getAttribute("value");
    ok("The button opens the existing recovery dialog with 「파일만」 preselected", chosen === "files" && await dialog.locator('input[type="radio"]').count() === 3, String(chosen));
    const asked = await page.evaluate(() => window.__CONTROLS__.map((one) => one.action.kind));
    ok("Opening it asks for a preview only, never a restore", asked.every((kind) => kind === "preview") && asked.length >= 1, JSON.stringify(asked));
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "detached" });
    await page.evaluate(() => {
      window.__FILES__ = { files_available: false, file_problem: "read-only" };
      closeConversationReview(document.querySelector(".pane-chat").__helperPage, false);
    });
    await page.locator(".pane-chat .chat-answer-review").click();
    await page.waitForFunction(() => window.__CONTROLS__.length >= 3);
    await page.evaluate(async () => { await window.__PAINTED__(); });
    const off = await page.evaluate(() => {
      const button = document.querySelector(".pane-chat .chat-review-undo");
      return { disabled: button?.disabled, tip: button?.dataset.tip, label: button?.getAttribute("aria-label") };
    });
    ok("Without the files scope the button is disabled and its tooltip is the dialog's own reason", off.disabled === true && off.tip === "읽기 전용 세션에서는 파일을 복원할 수 없습니다.", JSON.stringify(off));
    ok("Review undo raises no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

/* While the agent runs the composer says the person can choose when to send, and a waiting message says it goes next turn. */
export async function testConversationComposerNext(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openAfterScene(page);
    const look = () => page.evaluate(() => {
      const form = document.querySelector(".pane-chat .worker-composer");
      const next = form.querySelector(".worker-composer-next");
      return { placeholder: form.querySelector(".worker-composer-box").placeholder,
        next: next && !next.hidden ? next.textContent : null,
        beside: next?.previousElementSibling?.classList.contains("worker-composer-context") ?? false };
    });
    const idle = await look();
    ok("Idle, the composer keeps today's sentence", idle.placeholder.startsWith("다음 지시를 쓰세요") && idle.next === null, JSON.stringify(idle));
    await page.evaluate(async () => {
      const zo = agentRows.find((row) => row.id === "zo");
      Object.assign(zo, { steer_road: "channel", stop_door: true, interrupt_key: "Escape" });
      const term = activePaneOf(currentTab());
      hookStates.set(term, "working");
      paintPaneChat(term);
      paintComposerChipsFor(term);
      await window.__PAINTED__();
    });
    const running = await look();
    ok("Running with a queue/steer choice, the composer says the time to send can be chosen", running.placeholder === "다음 지시를 입력하세요. 실행 중에는 보낼 시점을 고를 수 있습니다." && running.next === null, JSON.stringify(running));
    const box = page.locator(".pane-chat .worker-composer-box");
    await box.click();
    await page.keyboard.type("다음 지시");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => (document.querySelector(".pane-chat .worker-composer").__workerRun.queue?.length ?? 0) === 1);
    await page.evaluate(async () => { await window.__PAINTED__(); });
    const queued = await look();
    ok("A message waiting for the next turn is said beside the context chip", queued.next === "다음 턴에 전송" && queued.beside, JSON.stringify(queued));
    ok("The composer's next-turn line raises no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

/* The T3 Code chat view (2026-11-08, window artifact p-86fbaa96b6d0fc7b, the person's six decisions), read
 * off the laid-out page on the product's folded view: the person's words a filled bubble on the right; a
 * settled turn keeps its last answer and folds its work and its in-between words into one quiet line that
 * opens to them, while the turn at work stays open; answers antialiased at a 1.65 leading; the composer one
 * card with no checkout band and a round 28px send; the head one line with the branch as a chip; the right
 * column's 「대화」 panel and its empty sentence; a quote of an answer's words into the composer; and
 * 「결과만」, which hides the work but shows it while find is open. */
export async function testConversationT3(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await openAfterScene(page);
    await foldConversations(page);
    await page.evaluate(async () => { await window.__PAINTED__(); await window.__PAINTED__(); });
    const settled = await page.evaluate(() => {
      // WCAG contrast of two `rgb()` colours, the second the ground.
      const channel = (value) => {
        const c = value / 255;
        return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
      };
      const luminance = (rgb) => 0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2]);
      const rgbOf = (value) => (value.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
      const contrastOf = (fore, back) => {
        const [a, b] = [luminance(rgbOf(fore)), luminance(rgbOf(back))].sort((x, y) => y - x);
        return (a + 0.05) / (b + 0.05);
      };
      const list = document.querySelector(".pane-chat .helper-turns");
      const folds = [...list.querySelectorAll(":scope > .helper-group")];
      const answers = [...list.querySelectorAll(":scope > .helper-turn.is-assistant")].map((row) => row.__turn.text);
      const cap = folds[0]?.querySelector(".helper-group-cap");
      const words = folds[0]?.querySelector(".helper-group-words");
      // The ground the line is read on: the nearest box that paints one.
      let ground = "rgb(0, 0, 0)";
      for (let node = list; node; node = node.parentElement) {
        const paint = getComputedStyle(node).backgroundColor;
        if (paint && paint !== "transparent" && !/^rgba\(.*,\s*0\)$/.test(paint)) {
          ground = paint;
          break;
        }
      }
      return {
        folds: folds.map((one) => ({ fold: one.classList.contains("is-turn-fold"), words: one.querySelector(".helper-group-words").textContent, members: one.__members.map((turn) => turn.role) })),
        answers,
        dot: cap ? getComputedStyle(cap, "::before").display : null,
        chevron: Boolean(cap?.querySelector(".helper-group-chevron")),
        ratio: words ? contrastOf(getComputedStyle(words).color, ground) : 0,
      };
    });
    ok("A settled turn keeps its last answer and folds its work and its in-between words into one line each (T3 decision 2)",
      settled.folds.length === 2 && settled.folds.every((one) => one.fold) &&
      settled.answers.length === 2 && !settled.answers.some((text) => text.startsWith("먼저 세션을 읽는")) &&
      settled.folds[1].members.includes("assistant") && settled.folds[1].members.filter((role) => role === "tool").length === 3,
      JSON.stringify(settled));
    ok("A settled turn's line says what the turn did, wears no dot and opens by a chevron, in words at 4.5:1 or more",
      settled.folds[1].words === "파일 읽기 1개 · 파일 수정 1개 · 외 1개" && settled.dot === "none" && settled.chevron && settled.ratio >= 4.5,
      JSON.stringify(settled));
    const opened = await page.evaluate(async () => {
      const fold = [...document.querySelectorAll(".pane-chat .helper-turns > .helper-group")].at(-1);
      fold.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      const body = fold.querySelector(":scope > .helper-group-body");
      const between = [...(body?.querySelectorAll(":scope > .helper-turn.is-assistant") ?? [])].map((row) => row.textContent.trim());
      // Read once the turn has eased in (the motion pair, 150 ms): `matrix(cos, sin, …)` of a quarter turn.
      await new Promise((done) => setTimeout(done, 300));
      const [cos, sin] = (getComputedStyle(fold.querySelector(".helper-group-chevron")).transform.match(/-?[\d.e-]+/g) ?? []).map(Number);
      const chevron = Math.abs(cos) < 0.01 && Math.abs(sin - 1) < 0.01;
      fold.querySelector(".helper-group-cap").click();
      await window.__PAINTED__();
      return { between, chevron, expanded: fold.querySelector(".helper-group-cap").getAttribute("aria-expanded") };
    });
    ok("Opened, the line stands the turn's in-between words with its steps, and its chevron turns down",
      opened.between.length === 1 && opened.between[0].startsWith("먼저 세션을 읽는") && opened.chevron === true && opened.expanded === "false",
      JSON.stringify(opened));
    const skin = await page.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const person = list.querySelector(":scope > .helper-turn.is-user > .helper-said");
      const row = person.parentElement;
      const answer = list.querySelector(":scope > .helper-turn.is-assistant > .helper-said");
      const said = getComputedStyle(person);
      const prose = getComputedStyle(answer);
      const send = document.querySelector(".pane-chat .worker-composer-send");
      const heading = document.querySelector(".pane-chat .chat-heading");
      const name = heading.querySelector(".worker-name");
      const branch = heading.querySelector(".chat-heading-branch");
      return {
        right: Math.abs(person.getBoundingClientRect().right - row.getBoundingClientRect().right),
        leftRoom: person.getBoundingClientRect().left - row.getBoundingClientRect().left,
        border: said.borderTopWidth,
        radius: said.borderTopLeftRadius,
        maxWidth: said.maxWidth,
        foot: Boolean(row.querySelector(":scope > .helper-person-foot > .helper-person-copy")),
        smoothing: prose.webkitFontSmoothing,
        leading: parseFloat(prose.lineHeight) / parseFloat(prose.fontSize),
        sendWidth: send.getBoundingClientRect().width,
        sendRadius: parseFloat(getComputedStyle(send).borderTopLeftRadius),
        band: document.querySelector(".pane-chat .chat-stack-git") !== null,
        stackShown: !(document.querySelector(".pane-chat .chat-stack")?.hidden ?? true),
        oneLine: Math.abs(name.getBoundingClientRect().top - branch.getBoundingClientRect().top) < 6 || branch.hidden,
        branch: branch.hidden ? null : branch.textContent,
      };
    });
    ok("The person's words stand as a filled bubble on the right — no outline, the floating radius, within 80% of the row — with their own line under them",
      skin.right < 2 && skin.leftRoom > 0 && skin.border === "0px" && skin.radius === "12px" && skin.maxWidth === "80%" && skin.foot,
      JSON.stringify(skin));
    ok("An answer reads antialiased at a 1.65 leading (T3 decision 3)",
      skin.smoothing === "antialiased" && Math.abs(skin.leading - 1.65) < 0.02, JSON.stringify(skin));
    ok("The composer is one card with a round 28px send, and the head is one line with the branch as a chip",
      !skin.band && !skin.stackShown && skin.sendWidth === 28 && skin.sendRadius >= 14 && skin.oneLine,
      JSON.stringify(skin));
    // The right column's 「대화」: the conversation in front, then the one sentence with none in front.
    const panel = await page.evaluate(async () => {
      document.getElementById("activity-chat-tab").click();
      await window.__PAINTED__();
      const host = document.getElementById("activity-chat");
      const seen = {
        shown: !host.hidden,
        selected: document.getElementById("activity-chat-tab").getAttribute("aria-selected"),
        cards: host.querySelectorAll(".chat-panel-card").length,
        text: host.textContent,
      };
      [...host.querySelectorAll(".chat-panel-door")].find((door) => door.textContent === "변경 보기")?.click();
      await window.__PAINTED__();
      seen.review = document.querySelector(".pane-chat .chat-review") !== null;
      document.querySelector(".pane-chat .chat-review .chat-answer-action:last-child")?.click();
      return seen;
    });
    ok("The right column's 「대화」 tells the conversation in front: the agent, the last turn's file and its tests, the checkout, the helpers — and opens the review",
      panel.shown && panel.selected === "true" && panel.cards >= 3 && panel.text.includes("src/session.rs") && panel.text.includes("+2 −1") &&
      panel.text.includes("✓ 테스트 3개 통과") && panel.review,
      JSON.stringify(panel));
    const picked = await page.evaluate(async () => {
      const pick = document.querySelector("#activity-chat .chat-panel-pick");
      if (!pick) return null;
      const options = [...pick.options].map((one) => one.textContent);
      pick.value = pick.options[0].value;
      pick.dispatchEvent(new Event("change"));
      await window.__PAINTED__();
      const text = document.getElementById("activity-chat").textContent;
      return { options, first: text.includes("docs.md") && !text.includes("src/session.rs") };
    });
    ok("With two turns that changed files, the panel picks whose changes it shows (T3 Code's 「Turn 2」)",
      picked?.options.join(",") === "차례 1,차례 2" && picked.first, JSON.stringify(picked));
    // A quote of an answer's words into the composer.
    const cited = await page.evaluate(async () => {
      const said = document.querySelector(".pane-chat .helper-turns > .helper-turn.is-assistant > .helper-said");
      const range = document.createRange();
      range.selectNodeContents(said);
      const chosen = document.getSelection();
      chosen.removeAllRanges();
      chosen.addRange(range);
      await new Promise((done) => setTimeout(done, 60));
      await window.__PAINTED__();
      const button = document.querySelector(".chat-cite");
      const shown = Boolean(button) && !button.hidden;
      button?.click();
      await window.__PAINTED__();
      const box = document.querySelector(".pane-chat .worker-composer-box");
      const seen = { shown, value: box.value, focus: document.activeElement === box, hidden: button?.hidden ?? null };
      box.value = "";
      box.dispatchEvent(new Event("input", { bubbles: true }));
      return seen;
    });
    ok("Words chosen in an answer raise 「인용해 묻기」, which puts them in the composer as a quote and sends nothing",
      cited.shown && cited.value.startsWith("> 문서 제목을 정리했습니다.\n\n") && cited.focus && cited.hidden === true,
      JSON.stringify(cited));
    // 「결과만」: the work goes out of sight, and stands again while find is open.
    const zen = await page.evaluate(async () => {
      const toggle = document.querySelector(".pane-chat .chat-answers-only");
      toggle.click();
      await window.__PAINTED__();
      const list = document.querySelector(".pane-chat .helper-turns");
      const shownFolds = () => [...list.querySelectorAll(":scope > .helper-group")].filter((one) => one.getBoundingClientRect().height > 0).length;
      const seen = { pressed: toggle.getAttribute("aria-pressed"), hidden: shownFolds() };
      const host = chatFindPage();
      openChatFind(host);
      await window.__PAINTED__();
      seen.finding = shownFolds();
      closeChatFind(host);
      await window.__PAINTED__();
      seen.after = shownFolds();
      toggle.click();
      await window.__PAINTED__();
      seen.back = shownFolds();
      seen.root = document.documentElement.dataset.chatAnswersOnly ?? null;
      return seen;
    });
    ok("「결과만」 hides the work and leaves the words, shows it again while find is open, and gives it back when turned off",
      zen.pressed === "true" && zen.hidden === 0 && zen.finding === 2 && zen.after === 0 && zen.back === 2 && zen.root === null,
      JSON.stringify(zen));
    // With no conversation in front, the panel says what it is for.
    const empty = await page.evaluate(async () => {
      await openTermTab({ placement: "tab" });
      await window.__PAINTED__();
      paintChatActivity();
      const host = document.getElementById("activity-chat");
      return { sentence: host.querySelector(".chat-panel-empty")?.textContent ?? null, cards: host.querySelectorAll(".chat-panel-card").length };
    });
    ok("With no conversation in front, 「대화」 says in one sentence what it is for",
      empty.sentence !== null && empty.cards === 0, JSON.stringify(empty));
    ok("The T3 chat view raises no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
  // The turn at work stays open: its in-between words and its pill stand, while the turn before it folds.
  const { page: working, faults: workFaults } = await openWindowTestPage(browser, origin);
  try {
    const turns = [
      ...afterTurns,
      { role: "user", text: "시험도 하나 더 써 줘.", message_index: afterTurns.length, session_id: "after-session" },
      { role: "assistant", text: "시험 파일을 먼저 읽겠습니다.", message_index: afterTurns.length + 1, session_id: "after-session" },
      { role: "tool", text: "Read src/session_test.rs", tool: { call_id: "more", name: "Read", kind: "read", input: "src/session_test.rs", file: { path: "src/session_test.rs" } }, message_index: afterTurns.length + 2, session_id: "after-session" },
    ];
    await openPaneConversation(working, turns, { agent: "claude" });
    await foldConversations(working);
    await working.evaluate(async () => { await window.__PAINTED__(); await window.__PAINTED__(); });
    const live = await working.evaluate(() => {
      const list = document.querySelector(".pane-chat .helper-turns");
      const rows = [...list.querySelectorAll(":scope > [data-turn]")];
      const last = rows.findLastIndex((row) => row.classList.contains("is-user"));
      return {
        after: rows.slice(last + 1).map((row) => row.classList.contains("helper-group") ? `pill${row.classList.contains("is-turn-fold") ? ":fold" : ""}` : row.classList.contains("is-assistant") ? "answer" : "other"),
        before: rows.slice(0, last).filter((row) => row.classList.contains("is-turn-fold")).length,
      };
    });
    ok("The turn at work stays open — its in-between words and its pill stand — while the settled turns before it fold",
      live.after.join(",") === "answer,pill" && live.before === 2, JSON.stringify(live));
    ok("The turn at work raises no page errors", workFaults.length === 0, workFaults.join("\n"));
  } finally { await working.close(); }
}

/* A conversation whose tab went behind another before its first frame — a tab opened and left at once, a
 * window restoring its tabs — measured its composer and its newest answer's buttons with no box
 * (2026-11-08, found checking the T3 view in a preview): the box kept `height: 0px` and stood squashed to
 * its padding, and the answer gave back the 44px it reserves for the buttons and took them again under a
 * list already placed at its foot, which then stood 44px short with the buttons under the composer. The
 * scene is the preview's: two settled turns in a window narrow enough that the list runs past its view. */
export async function testConversationBehindTab(browser, origin, ok) {
  const { page, faults } = await openWindowTestPage(browser, origin);
  try {
    await page.setViewportSize({ width: 608, height: 880 });
    await foldConversations(page);
    const seen = {};
    seen.waiting = await page.evaluate(async () => {
      const settle = async () => { await window.__PAINTED__(); await window.__PAINTED__(); };
      let at = 1_790_000_000_000;
      const stamp = (ms = 1800) => (at += ms);
      const call = (id, name, kind, input, extra = {}) => ({ role: "tool", text: `${name} · ${input}`, at_ms: stamp(), tool: { call_id: id, name, kind, input, is_error: false, ...extra } });
      const result = (id, text) => ({ role: "tool_result", text, at_ms: stamp(), tool: { call_id: id, name: "", input: "", is_error: false } });
      const reads = ["Gallery", "Carousel", "Viewer", "List", "Card", "Bundle"].flatMap((name, n) => [
        call(`r${n}`, "Read", "read", JSON.stringify({ file_path: `src/screens/${name}.tsx` })),
        result(`r${n}`, `     1→export const ${name} = () => null;`),
      ]);
      const turns = [
        { role: "user", text: "목록 화면이 묶음 상품을 보여 줄 수 있는지, 화면 폭을 고정값으로 쓰는 곳이 어디인지 찾아 줘.", at_ms: stamp() },
        { role: "thinking", text: "**목록 화면부터**\n폭 상수와 묶음 상품 필드를 찾는다.", at_ms: stamp() },
        call("g1", "Bash", "bash", "grep -rn SCREEN_WIDTH src"),
        result("g1", "src/screens/map/Carousel.tsx:16:const SCREEN_WIDTH = Dimensions.get('window').width"),
        ...reads,
        { role: "assistant", text: "두 곳이 화면 폭을 고정값으로 씁니다.\n\n- `Carousel.tsx` 16행: 창 폭을 한 번 읽어 카드 폭을 계산합니다.\n- `Viewer.tsx` 16행: 같은 방식입니다.\n\n```tsx\nconst SCREEN_WIDTH = Dimensions.get('window').width\nconst CARD_WIDTH = SCREEN_WIDTH - PADDING * 2\n```\n\n묶음 상품은 API가 이미 `bundle` 필드를 줍니다.", at_ms: stamp() },
        { role: "user", text: "Carousel 폭을 화면 회전에도 맞게 고쳐 줘. 시험도 돌려 줘.", at_ms: stamp(20_000) },
        { role: "assistant", text: "먼저 `Carousel.tsx`를 고치고 시험을 돌리겠습니다.", at_ms: stamp() },
        call("e1", "Edit", "edit", JSON.stringify({ file_path: "src/screens/map/Carousel.tsx" }), { edits: [{ path: "src/screens/map/Carousel.tsx", lines: [
          { kind: "del", text: "const SCREEN_WIDTH = Dimensions.get('window').width" },
          { kind: "add", text: "const { width: SCREEN_WIDTH } = useWindowDimensions()" },
        ] }] }),
        result("e1", "The file src/screens/map/Carousel.tsx has been updated."),
        call("b1", "Bash", "bash", "npm test -- Carousel"),
        result("b1", "PASS src/screens/map/Carousel.test.tsx\nTests: 3 passed, 3 total"),
        { role: "assistant", text: "`Carousel.tsx`가 이제 `useWindowDimensions()`로 폭을 읽어서, 화면을 돌리면 카드 폭도 다시 계산됩니다. 시험 3개가 통과했습니다.", at_ms: stamp() },
      ].map((turn, message_index) => ({ ...turn, message_index, session_id: "behind-tab" }));
      window.__ANSWER__.pane_log = (args) => ({ found: true, next: turns.length, turns: args?.after == null ? turns : turns.slice(args.after), skipped: false, more: false, folded: false, model: "claude-opus-5" });
      window.__behind = { front: activeTabIn(focusedPane).id };
      const term = await openTermTab({ placement: "tab" });
      window.__behind.term = term;
      paneAgents.set(term, "claude");
      hookStates.set(term, "done");
      await setPaneChat(term, true, { wire: false });
      await pollHelperPages();
      // The terminal tab comes back in the task that painted the conversation: its first frame finds it behind.
      setActiveTab(window.__behind.front);
      await settle();
      const box = document.querySelector(`.pane-slot[data-term="${term}"] .pane-chat .worker-composer-box`);
      return { shown: box.getClientRects().length > 0, inline: box.style.height };
    });
    // The right column's 「대화」 stands open, as the person reads beside the conversation.
    await page.click("#activity-chat-tab");
    const look = () => page.evaluate(() => {
      const chat = document.querySelector(`.pane-slot[data-term="${window.__behind.term}"] .pane-chat`);
      const box = chat.querySelector(".worker-composer-box");
      const list = chat.querySelector(".helper-turns");
      const answer = [...list.querySelectorAll(":scope > .helper-turn.is-assistant")].at(-1);
      const style = getComputedStyle(box);
      return {
        box: Math.round(box.getBoundingClientRect().height),
        line: Math.round(parseFloat(style.lineHeight) + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)),
        over: list.scrollHeight > list.clientHeight,
        gap: list.scrollHeight - Math.round(list.scrollTop) - list.clientHeight,
        answerBottom: Math.round(answer.getBoundingClientRect().bottom),
        dockTop: Math.round(chat.querySelector(".chat-dock").getBoundingClientRect().top),
        buttons: answer.querySelector(":scope > .helper-actions") !== null,
      };
    });
    const bring = async (which) => {
      await page.evaluate((key) => setActiveTab(key === "front" ? window.__behind.front : tabOfTerm(window.__behind.term).id), which);
      await page.evaluate(async () => { await window.__PAINTED__(); await window.__PAINTED__(); });
    };
    await bring("term");
    const back = await look();
    // Left for the terminal tab and brought back again: a settled conversation's plain round trip.
    await bring("front");
    await bring("term");
    const again = await look();
    Object.assign(seen, { back, again });
    ok("A composer whose conversation went behind another tab before its first frame measures nothing there, and brought forward stands one line tall",
      seen.waiting.shown === false && seen.waiting.inline !== "0px" && Math.abs(seen.back.box - seen.back.line) <= 1, JSON.stringify(seen));
    ok("Brought forward, its list stands at its foot with the newest answer and its buttons above the composer — and again after a round trip to the other tab",
      [seen.back, seen.again].every((one) => one.over && one.buttons && one.gap <= 1 && one.answerBottom <= one.dockTop), JSON.stringify(seen));
    ok("A conversation that goes behind another tab and comes back raises no page errors", faults.length === 0, faults.join("\n"));
  } finally { await page.close(); }
}

if (import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const at = process.argv.indexOf("--screenshots");
  const screenshots = at < 0 ? null : resolve(process.argv[at + 1]);
  if (screenshots) await mkdir(screenshots, { recursive: true });
  const { files, origin } = await createWindowServer();
  const browser = await chromium.launch({ headless: true });
  const failed = [];
  try {
    const report = (name, pass, details = "") => {
      console.log(`${pass ? "PASS" : "FAIL"} ${name}${pass ? "" : `\n${details}`}`);
      if (!pass) failed.push(name);
    };
    await testConversationAfter(browser, origin, report, screenshots);
    await testConversationArchive(browser, origin, report);
    await testConversationRecovery(browser, origin, report);
    await testConversationReviewTests(browser, origin, report);
    await testConversationReviewUndo(browser, origin, report);
    await testConversationComposerNext(browser, origin, report);
    await testConversationT3(browser, origin, report);
    await testConversationBehindTab(browser, origin, report);
  } finally { await browser.close(); files.close(); }
  assert.deepEqual(failed, []);
}
